//! 后台 task: 启动时 sweep *.tmp 残留 + interval + 双层 timeout + shutdown
//!
//! 复刻 main.rs:285-330 的 init_governance 模式 (spec §8.2).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::training_export::checkpoint::{self, Checkpoint};
use crate::training_export::config::TrainingExportConfig;
use crate::training_export::runner;
use crate::training_export::TriggerSource;

/// scheduler 关闭句柄 (模仿 GovernanceShutdown, main.rs:237-243)
pub struct TrainingExporterShutdown {
    pub shutdown_tx: watch::Sender<bool>,
    pub handle: JoinHandle<()>,
}

/// 启动训练导出后台 task.
pub fn start_training_exporter(
    config: TrainingExportConfig,
    pool: PgPool,
) -> TrainingExporterShutdown {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let handle = tokio::spawn(async move {
        run_scheduler_loop(config, pool, shutdown_rx).await;
    });
    TrainingExporterShutdown {
        shutdown_tx,
        handle,
    }
}

async fn run_scheduler_loop(
    config: TrainingExportConfig,
    pool: PgPool,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    // 启动时 sweep .tmp 残留 (spec §8.3)
    let data_dir = crate::paths::get_data_dir();
    let output_dir = data_dir.join(&config.paths.output_subdir);
    match checkpoint::sweep_tmp_files(&output_dir).await {
        Ok(n) if n > 0 => tracing::info!("启动 sweep: 清理 {} 个 .tmp 残留", n),
        _ => {}
    }

    let mut interval =
        tokio::time::interval(Duration::from_secs(config.scheduler.interval_secs));
    let run_timeout = Duration::from_secs(config.scheduler.run_timeout_secs);
    let is_running = Arc::new(AtomicBool::new(false));

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::info!("训练导出 task 收到关闭信号, 退出循环");
                    break;
                }
            }
            _ = interval.tick() => {
                if is_running.swap(true, Ordering::SeqCst) {
                    tracing::warn!("上次 run 仍在进行, 跳过本次触发");
                    continue;
                }
                let run_id = ulid::Ulid::new().to_string();
                let cfg_clone = config.clone();
                let pool_clone = pool.clone();
                let run_result = tokio::time::timeout(
                    run_timeout,
                    async {
                        let mut cp = load_checkpoint(&cfg_clone).await;
                        let now_date = chrono::Utc::now().format("%Y-%m-%d").to_string();
                        cp.retire_old_buckets(cfg_clone.checkpoint.retain_days, &now_date);
                        let result = runner::run_once(
                            &cfg_clone,
                            &pool_clone,
                            &mut cp,
                            TriggerSource::Scheduled,
                            run_id.clone(),
                        ).await;
                        if let Err(e) = cp.save(&checkpoint_path(&cfg_clone)).await {
                            tracing::warn!("checkpoint 保存失败: {}", e);
                        }
                        result
                    },
                ).await;

                is_running.store(false, Ordering::SeqCst);

                match run_result {
                    Ok(Ok(result)) => {
                        tracing::info!(
                            run_id = %result.metadata.run_id,
                            traces = result.metadata.trace_count,
                            samples = result.metadata.sample_count,
                            "训练导出 run 完成"
                        );
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(run_id = %run_id, "训练导出 run 失败: {}", e);
                    }
                    Err(_elapsed) => {
                        tracing::warn!(
                            run_id = %run_id,
                            "训练导出 run 超时 (>{:?}), 本次中止",
                            run_timeout
                        );
                    }
                }
            }
        }
    }
    tracing::info!("训练导出 task 已停止");
}

async fn load_checkpoint(config: &TrainingExportConfig) -> Checkpoint {
    let path = checkpoint_path(config);
    Checkpoint::load(&path).await.unwrap_or_default()
}

fn checkpoint_path(config: &TrainingExportConfig) -> std::path::PathBuf {
    crate::paths::get_data_dir().join(&config.paths.checkpoint_filename)
}
