//! 后台 task: 启动时 sweep *.tmp + scheduled/manual 单队列 + timeout + shutdown.

use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use futures_util::FutureExt;
use sqlx::PgPool;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::training_export::ExportRunRequest;
use crate::training_export::checkpoint::{self, Checkpoint};
use crate::training_export::config::TrainingExportConfig;
use crate::training_export::runner::{self, RunResult};

/// Scheduler 关闭句柄；main 持有并负责发送关闭信号与 join。
pub struct TrainingExporterShutdown {
    pub shutdown_tx: watch::Sender<bool>,
    pub handle: JoinHandle<()>,
}

/// 启动训练导出后台 task，并返回 manual queue sender 与关闭句柄。
pub fn start_training_exporter(
    config: TrainingExportConfig,
    pool: PgPool,
) -> (mpsc::Sender<ExportRunRequest>, TrainingExporterShutdown) {
    let (manual_tx, manual_rx) = mpsc::channel(config.scheduler.manual_queue_capacity);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let handle = tokio::spawn(async move {
        let result = AssertUnwindSafe(run_scheduler_loop(config, pool, shutdown_rx, manual_rx))
            .catch_unwind()
            .await;
        if let Err(payload) = result {
            tracing::error!(
                panic = %panic_payload_message(payload.as_ref()),
                "训练导出 scheduler 发生未捕获 panic, task 已隔离退出"
            );
        }
    });

    (
        manual_tx,
        TrainingExporterShutdown {
            shutdown_tx,
            handle,
        },
    )
}

async fn run_scheduler_loop(
    config: TrainingExportConfig,
    pool: PgPool,
    mut shutdown_rx: watch::Receiver<bool>,
    mut manual_rx: mpsc::Receiver<ExportRunRequest>,
) {
    let output_dir = crate::paths::get_data_dir().join(&config.paths.output_subdir);
    let checkpoint_file = checkpoint_path(&config);
    let checkpoint_dir = checkpoint_file.parent().map(std::path::Path::to_path_buf);
    for directory in std::iter::once(output_dir).chain(checkpoint_dir) {
        match checkpoint::sweep_tmp_files(&directory).await {
            Ok(removed) if removed > 0 => {
                tracing::info!(path = %directory.display(), removed, "训练导出启动 sweep 完成");
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(path = %directory.display(), "训练导出启动 sweep 失败: {}", error);
            }
        }
    }

    let mut interval = tokio::time::interval(Duration::from_secs(config.scheduler.interval_secs));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut manual_open = true;

    loop {
        let request = tokio::select! {
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    tracing::info!("训练导出 task 收到关闭信号, 退出循环");
                    break;
                }
                continue;
            }
            request = manual_rx.recv(), if manual_open => {
                match request {
                    Some(request) => request,
                    None => {
                        manual_open = false;
                        tracing::warn!("训练导出 manual queue 已关闭, 仅保留定时触发");
                        continue;
                    }
                }
            }
            _ = interval.tick() => {
                ExportRunRequest::scheduled(ulid::Ulid::new().to_string())
            }
        };

        if execute_request_with_shutdown(&config, &pool, request, &mut shutdown_rx).await {
            break;
        }
    }

    tracing::info!("训练导出 task 已停止");
}

/// 返回 true 表示收到 shutdown，scheduler 应退出。
async fn execute_request_with_shutdown(
    config: &TrainingExportConfig,
    pool: &PgPool,
    request: ExportRunRequest,
    shutdown_rx: &mut watch::Receiver<bool>,
) -> bool {
    let run_timeout = Duration::from_secs(config.scheduler.run_timeout_secs);
    // Cancel-aware atomic write 阶段 3：在 execute_request 与外层 select! 之间
    // 插入一个 CancellationToken。当前实现使用 watch::Receiver::changed() 表达
    // shutdown。run_future 内部的所有 await 都使用 await point，runtime 会在
    // `select!` 看到 shutdown 时通过 dropping timed_run 来立即返回；runner 内的
    // 同步 IO (`flush`/`sync_all`/`rename`) 不可中断，但 `reconcile_after_failure`
    // 通过 `try_exists(jsonl)` 保证最终一致性。
    let run_future = AssertUnwindSafe(execute_request(config, pool, &request)).catch_unwind();
    let timed_run = tokio::time::timeout(run_timeout, run_future);
    tokio::pin!(timed_run);

    loop {
        tokio::select! {
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    // Cancel-aware atomic write 阶段 4：取消时先记录当前是否已落盘
                    // jsonl，再调用 `reconcile_after_failure` 决定写 Completed 或 Failed。
                    let reason = "server shutdown 取消训练导出 run";
                    if let Ok(false) = jsonl_already_committed(config, &request).await {
                        tracing::info!(
                            run_id = %request.run_id,
                            "shutdown 取消但 jsonl 尚未落盘, 走 reconcile_after_failure 兜底"
                        );
                    }
                    record_failure(config, &request, reason).await;
                    tracing::info!(run_id = %request.run_id, "{}", reason);
                    return true;
                }
            }
            result = &mut timed_run => {
                match result {
                    Ok(Ok(Ok(run_result))) => log_success(&run_result),
                    Ok(Ok(Err(error))) => {
                        let reason = format!("训练导出 run 失败: {error:#}");
                        record_failure(config, &request, &reason).await;
                        tracing::warn!(run_id = %request.run_id, "{}", reason);
                    }
                    Ok(Err(payload)) => {
                        let reason = format!(
                            "训练导出 run panic: {}",
                            panic_payload_message(payload.as_ref())
                        );
                        record_failure(config, &request, &reason).await;
                        tracing::warn!(run_id = %request.run_id, "{}", reason);
                    }
                    Err(_) => {
                        let reason = format!(
                            "训练导出 run 超时 (>{}s)",
                            config.scheduler.run_timeout_secs
                        );
                        record_failure(config, &request, &reason).await;
                        tracing::warn!(run_id = %request.run_id, "{}", reason);
                    }
                }
                return false;
            }
        }
    }
}

/// 在 cancel 路径上预先确认 jsonl 是否已落盘。该 await 单独 try 一次失败也
/// 不会中断 reconcile_after_failure 的 try_exists 调用，因此重复检查是安全的。
async fn jsonl_already_committed(
    config: &TrainingExportConfig,
    request: &ExportRunRequest,
) -> anyhow::Result<bool> {
    let jsonl_path = crate::paths::get_data_dir()
        .join(&config.paths.output_subdir)
        .join(format!("run={}.jsonl", request.run_id));
    tokio::fs::try_exists(&jsonl_path)
        .await
        .with_context(|| format!("检查训练导出 jsonl 失败: {}", jsonl_path.display()))
}

async fn execute_request(
    config: &TrainingExportConfig,
    pool: &PgPool,
    request: &ExportRunRequest,
) -> anyhow::Result<RunResult> {
    let checkpoint_path = checkpoint_path(config);
    let mut checkpoint = Checkpoint::load(&checkpoint_path)
        .await
        .with_context(|| format!("加载 checkpoint 失败: {}", checkpoint_path.display()))?;
    let now_date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    checkpoint.retire_old_buckets(config.checkpoint.retain_days, &now_date);
    checkpoint.truncate_buckets(config.scheduler.max_bucket_size);

    let result = runner::run_once(config, pool, &mut checkpoint, request).await?;
    checkpoint.last_run_at = Some(chrono::Utc::now().timestamp_millis());
    if let Err(error) = checkpoint.save(&checkpoint_path).await {
        // 产物已原子提交；checkpoint 失败最多导致下轮重复导出，不能把成功 run 改写为 failed。
        tracing::warn!(
            run_id = %request.run_id,
            path = %checkpoint_path.display(),
            "保存 checkpoint 失败, 下轮可能重复处理: {error:#}"
        );
    }
    Ok(result)
}

async fn record_failure(config: &TrainingExportConfig, request: &ExportRunRequest, reason: &str) {
    if let Err(error) = runner::reconcile_after_failure(config, request, reason).await {
        tracing::error!(
            run_id = %request.run_id,
            reason,
            "写训练导出 reconcile_after_failure 失败: {error:#}"
        );
    }
}

fn log_success(result: &RunResult) {
    tracing::info!(
        run_id = %result.metadata.run_id,
        traces = result.metadata.trace_count,
        samples = result.metadata.sample_count,
        output_size_bytes = result.metadata.output_size_bytes,
        "训练导出 run 完成"
    );
}

fn checkpoint_path(config: &TrainingExportConfig) -> PathBuf {
    crate::paths::get_data_dir().join(&config.paths.checkpoint_filename)
}

fn panic_payload_message(payload: &(dyn Any + Send)) -> &str {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        message
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.as_str()
    } else {
        "non-string panic payload"
    }
}

#[cfg(test)]
mod tests {
    use super::panic_payload_message;

    #[test]
    fn panic_payload_string_is_preserved() {
        let payload: Box<dyn std::any::Any + Send> = Box::new(String::from("boom"));
        assert_eq!(panic_payload_message(payload.as_ref()), "boom");
    }

    #[tokio::test]
    async fn timeout_after_jsonl_committed_preserves_completed_meta() {
        use crate::training_export::runner;
        use crate::training_export::{ExportRunRequest, RunStatus, TriggerSource};

        let temp = tempfile::tempdir().expect("tempdir");
        let data_root = temp.path().to_path_buf();

        let run_id = ulid::Ulid::new().to_string();
        let output_dir = data_root.join("training_exports/sft");
        tokio::fs::create_dir_all(&output_dir).await.unwrap();
        tokio::fs::write(
            output_dir.join(format!("run={run_id}.jsonl")),
            b"{\"sample\":1}\n",
        )
        .await
        .unwrap();

        let mut config = crate::training_export::config::TrainingExportConfig::default();
        config.paths.output_subdir = "training_exports/sft".to_string();
        let request = ExportRunRequest {
            run_id: run_id.clone(),
            triggered_by: TriggerSource::Manual,
            agent_id_filter: None,
            force_full: false,
        };

        let result = runner::reconcile_after_failure_at(
            &data_root,
            &config,
            &request,
            "server shutdown 取消训练导出 run",
        )
        .await
        .expect("reconcile");
        assert_eq!(result.status, RunStatus::Completed);
        let metadata_path = output_dir.join(format!("run={run_id}.meta.json"));
        let meta_content = tokio::fs::read_to_string(&metadata_path)
            .await
            .expect("meta exists after cancel");
        let metadata: serde_json::Value = serde_json::from_str(&meta_content).expect("meta parses");
        assert_eq!(metadata["status"], "completed");
        assert!(
            metadata["error"]
                .as_str()
                .unwrap_or_default()
                .contains("取消")
        );
    }
}
