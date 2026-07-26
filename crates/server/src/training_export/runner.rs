//! 单次训练导出编排: trace 扫描 → DB 审查关联 → SFT 转换 → 原子产物写入.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Context;
use cyber_jianghu_protocol::{SoulCycleMetadata, TraceEntry};
use sqlx::PgPool;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::training_export::checkpoint::Checkpoint;
use crate::training_export::config::TrainingExportConfig;
use crate::training_export::sft_transform::{SftSample, TransformInput, transform_entry};
use crate::training_export::{ExportRunRequest, RunMetadata, RunStatus, validate_run_id};

const BYTES_PER_GIB: u64 = 1024 * 1024 * 1024;

/// 从 DB 查每个 (agent_id, tick_id) 的 soul_cycle_metadata (取最大 pipe_seq).
///
/// SQL 对齐 scripts/build_sft_data.py:84-92 (DISTINCT ON + pipe_seq DESC).
/// IN 子句用 UNNEST($1::uuid[], $2::bigint[]) 避免 sqlx 复合类型映射 (spec §5.3.1).
/// statement_timeout 用 SET LOCAL 在短事务内 (防 GUC 泄漏, spec §5.3.1).
pub async fn fetch_soul_cycle_metadata(
    pool: &PgPool,
    keys: &[(Uuid, i64)],
    statement_timeout_secs: u64,
) -> anyhow::Result<HashMap<(Uuid, i64), SoulCycleMetadata>> {
    if keys.is_empty() {
        return Ok(HashMap::new());
    }

    let agent_ids: Vec<Uuid> = keys.iter().map(|(agent_id, _)| *agent_id).collect();
    let tick_ids: Vec<i64> = keys.iter().map(|(_, tick_id)| *tick_id).collect();

    let mut tx = pool.begin().await.context("开始训练导出查询事务失败")?;
    // 参数化绑定, 与下方 SELECT 的 $1/$2 风格一致 (避免 SQL 格式化字符串).
    // statement_timeout_secs 是 u64, 且 config validate 强校验 > 0, 不可注入.
    let timeout_value = format!("{}s", statement_timeout_secs);
    sqlx::query("SET LOCAL statement_timeout = $1")
        .bind(&timeout_value)
        .execute(&mut *tx)
        .await
        .context("SET LOCAL statement_timeout 失败")?;

    let rows = sqlx::query_as::<_, SoulCycleRow>(
        r#"
        SELECT DISTINCT ON (agent_id, tick_id)
               agent_id, tick_id, soul_cycle_metadata
        FROM agent_action_logs
        WHERE soul_cycle_metadata IS NOT NULL
          AND (agent_id, tick_id) IN (
              SELECT * FROM UNNEST($1::uuid[], $2::bigint[])
          )
        ORDER BY agent_id, tick_id, pipe_seq DESC
        "#,
    )
    .bind(&agent_ids)
    .bind(&tick_ids)
    .fetch_all(&mut *tx)
    .await
    .context("查询 soul_cycle_metadata 失败")?;

    tx.commit().await.context("提交只读事务失败")?;

    let mut map = HashMap::with_capacity(rows.len());
    for row in rows {
        if let Some(metadata_value) = row.soul_cycle_metadata {
            match serde_json::from_value::<SoulCycleMetadata>(metadata_value) {
                Ok(metadata) => {
                    map.insert((row.agent_id, row.tick_id), metadata);
                }
                Err(error) => {
                    tracing::warn!(
                        agent_id = %row.agent_id,
                        tick_id = row.tick_id,
                        "解析 soul_cycle_metadata 失败: {}",
                        error
                    );
                }
            }
        }
    }
    Ok(map)
}

#[derive(sqlx::FromRow)]
struct SoulCycleRow {
    agent_id: Uuid,
    tick_id: i64,
    soul_cycle_metadata: Option<serde_json::Value>,
}

/// 单次 run 的结果
pub struct RunResult {
    pub metadata: RunMetadata,
    pub samples: Vec<SftSample>,
}

/// 执行一次完整 run (spec §4.1 五步).
pub async fn run_once(
    config: &TrainingExportConfig,
    pool: &PgPool,
    checkpoint: &mut Checkpoint,
    request: &ExportRunRequest,
) -> anyhow::Result<RunResult> {
    validate_run_id(&request.run_id).context("训练导出 request.run_id 不是合法 ULID")?;
    let data_dir = crate::paths::get_data_dir();
    let traces_dir = data_dir.join(&config.paths.traces_input_subdir);
    let output_dir = data_dir.join(&config.paths.output_subdir);

    let mut metadata = RunMetadata::for_request(request);
    metadata.status = RunStatus::Running;

    let (entries, keys, trace_dates) = scan_trace_files(
        &traces_dir,
        config.limits.max_traces_per_run,
        request.agent_id_filter,
    )
    .await?;
    metadata.trace_count = entries.len();

    let audit_map = if keys.is_empty() {
        HashMap::new()
    } else {
        fetch_audit_map_batched(
            pool,
            &keys,
            config.limits.db_batch_size,
            config.limits.db_statement_timeout_secs,
        )
        .await?
    };

    let mut samples = Vec::new();
    let yield_every = config.limits.yield_every_n.max(1);
    for (index, (entry, date)) in entries.iter().zip(trace_dates.iter()).enumerate() {
        if !request.force_full && checkpoint.is_processed(date, &entry.trace_id) {
            continue;
        }

        let Some(tianhun_result) = lookup_attempt_match(entry, &audit_map) else {
            // DB 尚未写入/attempt 尚未审查, 保留未处理状态供下轮重试.
            continue;
        };

        if tianhun_result == "approved"
            && let Some(sample) = transform_entry(TransformInput {
                entry,
                tianhun_result: Some(tianhun_result),
            })
        {
            samples.push(sample);
        }

        // 已有明确天魂结果时，本 trace 已完成处理；rejected/无效响应无需重复查询。
        checkpoint.mark_processed(
            date,
            entry.trace_id.clone(),
            config.scheduler.max_bucket_size,
        );

        if index % yield_every == 0 {
            tokio::task::yield_now().await;
        }
    }

    metadata.sample_count = samples.len();

    let mut content = String::new();
    for sample in &samples {
        content.push_str(&serde_json::to_string(sample)?);
        content.push('\n');
    }

    enforce_output_size_limit(
        &output_dir,
        u64::try_from(content.len()).context("训练导出内容大小超出 u64")?,
        config.limits.max_total_export_size_gb,
    )
    .await?;

    tokio::fs::create_dir_all(&output_dir)
        .await
        .with_context(|| format!("创建训练导出目录失败: {}", output_dir.display()))?;
    let output_path = output_dir.join(format!("run={}.jsonl", request.run_id));
    // Cancel-aware atomic write 阶段 1：先写 jsonl 到 .tmp，再 await commit rename。
    // 此 await 是唯一能安全观察到 shutdown 的同步点。
    write_atomic(&output_path, content.as_bytes(), true).await?;

    metadata.output_path = output_path
        .strip_prefix(&data_dir)
        .unwrap_or(&output_path)
        .to_string_lossy()
        .to_string();
    metadata.output_size_bytes =
        u64::try_from(content.len()).context("训练导出内容大小超出 u64")?;
    metadata.status = RunStatus::Completed;
    metadata.completed_at = Some(chrono::Utc::now().timestamp_millis());
    // Cancel-aware atomic write 阶段 2：再写 meta。如果此 await 之前/期间 runtime
    // 已经被 `tokio::time::timeout` 取消，我们依赖外部 `reconcile_after_failure`
    // 通过 `try_exists(jsonl)` 兜底保持一致性。这里仍然要写 meta，因为单元测试
    // 路径不会触发外部兜底。
    write_metadata(config, &metadata).await?;

    Ok(RunResult { metadata, samples })
}

/// 同上，但支持测试传入显式 data_root 避免与全局 env 竞争。
pub async fn write_failed_metadata_at(
    data_root: &Path,
    config: &TrainingExportConfig,
    request: &ExportRunRequest,
    error: impl Into<String>,
) -> anyhow::Result<RunMetadata> {
    validate_run_id(&request.run_id).context("训练导出 request.run_id 不是合法 ULID")?;
    let existing_path = data_root
        .join(&config.paths.output_subdir)
        .join(format!("run={}.meta.json", request.run_id));
    match tokio::fs::read_to_string(&existing_path).await {
        Ok(content) => {
            let metadata = serde_json::from_str::<RunMetadata>(&content).with_context(|| {
                format!("解析已有训练导出元数据失败: {}", existing_path.display())
            })?;
            anyhow::ensure!(
                metadata.run_id == request.run_id,
                "已有训练导出元数据 run_id 与请求不一致"
            );
            return Ok(metadata);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!("读取已有训练导出元数据失败: {}", existing_path.display())
            });
        }
    }

    let mut metadata = RunMetadata::for_request(request);
    metadata.status = RunStatus::Failed;
    metadata.completed_at = Some(chrono::Utc::now().timestamp_millis());
    metadata.error = Some(error.into());
    write_metadata_at(data_root, config, &metadata).await?;
    Ok(metadata)
}

/// 失败/取消时按最终一致性检查：若 jsonl 已落盘但 meta 未写或为 Failed，
/// 优先保留 jsonl 并补写 Completed 风格 meta；jsonl 未落盘时直接写 Failed。
/// 这一约定让 cancel/timeout 不会留下"jsonl 与 meta 状态不一致"的双写竞争。
pub async fn reconcile_after_failure(
    config: &TrainingExportConfig,
    request: &ExportRunRequest,
    error: impl Into<String>,
) -> anyhow::Result<RunMetadata> {
    reconcile_after_failure_at(&crate::paths::get_data_dir(), config, request, error).await
}

/// 同上，但支持测试传入显式 data_root 避免与全局 env 竞争。
pub async fn reconcile_after_failure_at(
    data_root: &Path,
    config: &TrainingExportConfig,
    request: &ExportRunRequest,
    error: impl Into<String>,
) -> anyhow::Result<RunMetadata> {
    validate_run_id(&request.run_id).context("训练导出 request.run_id 不是合法 ULID")?;
    let output_dir = data_root.join(&config.paths.output_subdir);
    let jsonl_path = output_dir.join(format!("run={}.jsonl", request.run_id));
    let jsonl_exists = tokio::fs::try_exists(&jsonl_path)
        .await
        .with_context(|| format!("检查训练导出 jsonl 失败: {}", jsonl_path.display()))?;
    if jsonl_exists {
        let mut metadata = RunMetadata::for_request(request);
        metadata.status = RunStatus::Completed;
        metadata.completed_at = Some(chrono::Utc::now().timestamp_millis());
        metadata.output_path = jsonl_path
            .strip_prefix(data_root)
            .unwrap_or(&jsonl_path)
            .to_string_lossy()
            .to_string();
        if let Ok(attributes) = tokio::fs::metadata(&jsonl_path).await {
            metadata.output_size_bytes = attributes.len();
        }
        metadata.error = Some(error.into());
        write_metadata_at(data_root, config, &metadata).await?;
        return Ok(metadata);
    }
    write_failed_metadata_at(data_root, config, request, error).await
}

async fn write_metadata(
    config: &TrainingExportConfig,
    metadata: &RunMetadata,
) -> anyhow::Result<()> {
    write_metadata_at(&crate::paths::get_data_dir(), config, metadata).await
}

pub async fn write_metadata_at(
    data_root: &Path,
    config: &TrainingExportConfig,
    metadata: &RunMetadata,
) -> anyhow::Result<()> {
    validate_run_id(&metadata.run_id).context("训练导出 metadata.run_id 不是合法 ULID")?;
    let output_dir = data_root.join(&config.paths.output_subdir);
    tokio::fs::create_dir_all(&output_dir)
        .await
        .with_context(|| format!("创建训练导出目录失败: {}", output_dir.display()))?;
    let meta_path = output_dir.join(format!("run={}.meta.json", metadata.run_id));
    let content = serde_json::to_vec_pretty(metadata).context("序列化训练导出元数据失败")?;
    write_atomic(&meta_path, &content, false).await
}

async fn write_atomic(path: &Path, content: &[u8], sync: bool) -> anyhow::Result<()> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("训练导出目标文件名不是有效 UTF-8")?;
    let tmp_path = path.with_file_name(format!("{file_name}.tmp"));

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp_path)
        .await
        .with_context(|| format!("创建训练导出临时文件失败: {}", tmp_path.display()))?;
    file.write_all(content)
        .await
        .with_context(|| format!("写训练导出临时文件失败: {}", tmp_path.display()))?;
    file.flush()
        .await
        .with_context(|| format!("刷新训练导出临时文件失败: {}", tmp_path.display()))?;
    if sync {
        file.sync_all()
            .await
            .with_context(|| format!("fsync 训练导出临时文件失败: {}", tmp_path.display()))?;
    }
    drop(file);

    tokio::fs::rename(&tmp_path, path).await.with_context(|| {
        format!(
            "原子替换训练导出文件失败: {} -> {}",
            tmp_path.display(),
            path.display()
        )
    })?;
    Ok(())
}

async fn enforce_output_size_limit(
    output_dir: &Path,
    pending_bytes: u64,
    max_total_export_size_gb: u64,
) -> anyhow::Result<()> {
    let max_bytes = max_total_export_size_gb
        .checked_mul(BYTES_PER_GIB)
        .context("训练导出目录大小上限溢出")?;
    let current_bytes = directory_size_bytes(output_dir).await?;
    let projected_bytes = current_bytes
        .checked_add(pending_bytes)
        .context("训练导出目录预计大小溢出")?;
    anyhow::ensure!(
        projected_bytes <= max_bytes,
        "训练导出目录将超限: projected={} bytes, limit={} bytes",
        projected_bytes,
        max_bytes
    );
    Ok(())
}

async fn directory_size_bytes(dir: &Path) -> anyhow::Result<u64> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(error).with_context(|| format!("读取训练导出目录失败: {}", dir.display()));
        }
    };

    let mut total = 0_u64;
    while let Some(entry) = entries
        .next_entry()
        .await
        .with_context(|| format!("遍历训练导出目录失败: {}", dir.display()))?
    {
        let metadata = entry
            .metadata()
            .await
            .with_context(|| format!("读取训练导出文件元数据失败: {}", entry.path().display()))?;
        if metadata.is_file() {
            total = total
                .checked_add(metadata.len())
                .context("训练导出目录大小求和溢出")?;
        }
    }
    Ok(total)
}

async fn scan_trace_files(
    traces_dir: &Path,
    max_traces: usize,
    agent_id_filter: Option<Uuid>,
) -> anyhow::Result<(Vec<TraceEntry>, Vec<(Uuid, i64)>, Vec<String>)> {
    let mut entries = Vec::new();
    let mut keys = HashSet::new();
    let mut dates = Vec::new();

    let mut agent_dirs = match tokio::fs::read_dir(traces_dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((entries, Vec::new(), dates));
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("读取 trace 目录失败: {}", traces_dir.display()));
        }
    };

    while let Some(agent_entry) = agent_dirs
        .next_entry()
        .await
        .with_context(|| format!("遍历 trace agent 目录失败: {}", traces_dir.display()))?
    {
        if !agent_entry
            .file_type()
            .await
            .with_context(|| format!("读取目录类型失败: {}", agent_entry.path().display()))?
            .is_dir()
        {
            continue;
        }

        let mut date_files = match tokio::fs::read_dir(agent_entry.path()).await {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!(
                    path = %agent_entry.path().display(),
                    "读取 trace agent 目录失败, 跳过: {}",
                    error
                );
                continue;
            }
        };

        while let Some(date_entry) = date_files
            .next_entry()
            .await
            .with_context(|| format!("遍历 trace 日期文件失败: {}", agent_entry.path().display()))?
        {
            let path = date_entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
                continue;
            }

            let Some(date) = trace_date_from_path(&path) else {
                tracing::warn!(path = %path.display(), "trace 文件名缺少 date=YYYY-MM-DD, 跳过");
                continue;
            };
            let file = match tokio::fs::File::open(&path).await {
                Ok(file) => file,
                Err(error) => {
                    tracing::warn!(path = %path.display(), "读取 trace 文件失败, 跳过: {}", error);
                    continue;
                }
            };
            let reached_limit = stream_parse_trace_lines(
                file,
                &path,
                &date,
                agent_id_filter,
                max_traces,
                &mut entries,
                &mut keys,
                &mut dates,
            )
            .await?;
            if reached_limit {
                break;
            }
        }
        if entries.len() >= max_traces {
            break;
        }
    }

    Ok((entries, keys.into_iter().collect(), dates))
}

#[allow(clippy::too_many_arguments)]
async fn stream_parse_trace_lines(
    file: tokio::fs::File,
    path: &Path,
    date: &str,
    agent_id_filter: Option<Uuid>,
    max_traces: usize,
    entries: &mut Vec<TraceEntry>,
    keys: &mut HashSet<(Uuid, i64)>,
    dates: &mut Vec<String>,
) -> anyhow::Result<bool> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut reached_limit = false;
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .await
            .with_context(|| format!("读取 trace 行失败: {}", path.display()))?;
        if read == 0 {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        if entries.len() >= max_traces {
            reached_limit = true;
            break;
        }
        match serde_json::from_str::<TraceEntry>(&line) {
            Ok(entry)
                if agent_id_filter
                    .is_none_or(|filter_agent_id| entry.agent_id == filter_agent_id) =>
            {
                keys.insert((entry.agent_id, entry.tick_id));
                entries.push(entry);
                dates.push(date.to_string());
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(path = %path.display(), "解析 trace 行失败, 跳过: {}", error);
            }
        }
    }
    Ok(reached_limit)
}

fn trace_date_from_path(path: &Path) -> Option<String> {
    let date = path
        .file_stem()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("date="))?;
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .map(|_| date.to_string())
}

#[cfg(test)]
mod scan_tests {
    use super::scan_trace_files;
    use uuid::Uuid;

    #[tokio::test]
    async fn scan_returns_empty_arrays_when_dir_absent() {
        // 若 traces_dir 不存在，scan 必须返回 (Vec, Vec, Vec)，run_once 的
        // if keys.is_empty() 分支会直接进入 empty-run 写 Completed meta 路径。
        let temp = tempfile::tempdir().expect("tempdir");
        let missing = temp.path().join("does-not-exist");
        let (entries, keys, dates) = scan_trace_files(&missing, 10, None)
            .await
            .expect("scan trace files on missing dir");
        assert!(entries.is_empty());
        assert!(keys.is_empty());
        assert!(dates.is_empty());
    }

    #[tokio::test]
    async fn scan_drops_entries_mismatching_agent_filter() {
        // 准备 agent 目录与 trace 文件，agent_id_filter 不匹配时 entry 应被过滤。
        let temp = tempfile::tempdir().expect("tempdir");
        let traces_dir = temp.path().join("traces/soul=renhun");
        let agent_id_keep = Uuid::new_v4();
        let agent_id_drop = Uuid::new_v4();
        let keep_dir = traces_dir.join(format!("agent={agent_id_keep}"));
        let drop_dir = traces_dir.join(format!("agent={agent_id_drop}"));
        tokio::fs::create_dir_all(&keep_dir).await.unwrap();
        tokio::fs::create_dir_all(&drop_dir).await.unwrap();
        tokio::fs::write(
            keep_dir.join("date=2026-07-26.jsonl"),
            format!("{{\"trace_id\":\"a\",\"agent_id\":\"{agent_id_keep}\",\"character_name\":\"k\",\"tick_id\":1,\"soul_stage\":\"Renhun\",\"attempt\":0,\"provider\":\"p\",\"model\":\"m\",\"persona_name\":\"\",\"persona_description\":\"\",\"user_prompt\":\"u\",\"response\":\"r\",\"ok\":true,\"wall_clock\":1}}\n"),
        )
        .await
        .unwrap();
        tokio::fs::write(
            drop_dir.join("date=2026-07-26.jsonl"),
            format!("{{\"trace_id\":\"b\",\"agent_id\":\"{agent_id_drop}\",\"character_name\":\"d\",\"tick_id\":2,\"soul_stage\":\"Renhun\",\"attempt\":0,\"provider\":\"p\",\"model\":\"m\",\"persona_name\":\"\",\"persona_description\":\"\",\"user_prompt\":\"u\",\"response\":\"r\",\"ok\":true,\"wall_clock\":1}}\n"),
        )
        .await
        .unwrap();

        let (entries, _keys, _dates) = scan_trace_files(&traces_dir, 100, Some(agent_id_keep))
            .await
            .expect("scan with filter");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].agent_id, agent_id_keep);
        // 对照：不带 filter 时两条都被收集。
        let (entries_all, _, _) = scan_trace_files(&traces_dir, 100, None)
            .await
            .expect("scan without filter");
        assert_eq!(entries_all.len(), 2);
    }

    #[tokio::test]
    async fn scan_skip_non_jsonl_and_invalid_date() {
        let temp = tempfile::tempdir().expect("tempdir");
        let traces_dir = temp.path().join("traces/soul=renhun");
        let agent_dir = traces_dir.join(format!("agent={}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&agent_dir).await.unwrap();
        tokio::fs::write(agent_dir.join("date=2026-07-26.jsonl"), "{}\n")
            .await
            .unwrap();
        tokio::fs::write(agent_dir.join("date=2026-07-26.json"), "{}\n")
            .await
            .unwrap();
        tokio::fs::write(agent_dir.join("date=2026-99-99.jsonl"), "{}\n")
            .await
            .unwrap();
        tokio::fs::write(agent_dir.join("README"), "noise")
            .await
            .unwrap();
        let (entries, _, _) = scan_trace_files(&traces_dir, 100, None)
            .await
            .expect("scan");
        assert!(entries.is_empty());
    }
}

async fn fetch_audit_map_batched(
    pool: &PgPool,
    keys: &[(Uuid, i64)],
    batch_size: usize,
    statement_timeout_secs: u64,
) -> anyhow::Result<HashMap<(Uuid, i64), SoulCycleMetadata>> {
    let mut total = HashMap::new();
    for chunk in keys.chunks(batch_size.max(1)) {
        let part = fetch_soul_cycle_metadata(pool, chunk, statement_timeout_secs).await?;
        total.extend(part);
    }
    Ok(total)
}

fn lookup_attempt_match(
    entry: &TraceEntry,
    audit_map: &HashMap<(Uuid, i64), SoulCycleMetadata>,
) -> Option<String> {
    let metadata = audit_map.get(&(entry.agent_id, entry.tick_id))?;
    let cycle = metadata
        .cycles
        .iter()
        .find(|cycle| cycle.attempt == entry.attempt)?;
    cycle.tianhun.result.clone()
}

#[cfg(test)]
mod tests {
    use super::{lookup_attempt_match, trace_date_from_path};
    use std::collections::HashMap;
    use std::path::Path;

    use cyber_jianghu_protocol::{
        RenhunReport, SoulCycleAttempt, SoulCycleMetadata, TianhunReport, TraceEntry,
    };
    use uuid::Uuid;

    #[test]
    fn trace_date_is_extracted_from_partition_file() {
        assert_eq!(
            trace_date_from_path(Path::new("date=2026-07-26.jsonl")),
            Some("2026-07-26".to_string())
        );
    }

    #[test]
    fn invalid_partition_name_is_rejected() {
        assert_eq!(trace_date_from_path(Path::new("traces.jsonl")), None);
        assert_eq!(
            trace_date_from_path(Path::new("date=2026-99-99.jsonl")),
            None
        );
    }

    // ---- lookup_attempt_match fixture 测试 (spec §11 验收 #1) ----
    // spec §4.4: attempt 精确匹配是有意偏离 Python 的核心逻辑, 必须独立 fixture 覆盖.

    fn make_trace(agent: Uuid, tick: i64, attempt: i32) -> TraceEntry {
        TraceEntry {
            trace_id: format!("test-{}-{}-{}", agent, tick, attempt),
            agent_id: agent,
            character_name: "TestAgent".to_string(),
            tick_id: tick,
            soul_stage: "Renhun".to_string(),
            attempt,
            provider: "test".to_string(),
            model: "test-model".to_string(),
            persona_name: "测试".to_string(),
            persona_description: "描述".to_string(),
            user_prompt: "提示".to_string(),
            response: "回复".to_string(),
            prompt_tokens: None,
            completion_tokens: None,
            ok: true,
            wall_clock: None,
        }
    }

    fn cycle(attempt: i32, result: Option<&str>) -> SoulCycleAttempt {
        SoulCycleAttempt {
            attempt,
            renhun: RenhunReport {
                narrative: None,
                thought_log: None,
                earth_tool_calls: None,
            },
            tianhun: TianhunReport {
                result: result.map(String::from),
                layers: vec![],
                reason: None,
            },
            final_intent: None,
            model_id: None,
        }
    }

    fn metadata(cycles: Vec<SoulCycleAttempt>) -> SoulCycleMetadata {
        SoulCycleMetadata {
            world_time: None,
            cycles,
            immediate_intents: vec![],
        }
    }

    #[test]
    fn single_attempt_approved_returns_approved() {
        // 场景: trace 是 attempt=0, cycles 只有 attempt=0 且 approved
        let agent = Uuid::nil();
        let trace = make_trace(agent, 100, 0);
        let mut map = HashMap::new();
        map.insert((agent, 100), metadata(vec![cycle(0, Some("approved"))]));

        assert_eq!(
            lookup_attempt_match(&trace, &map),
            Some("approved".to_string())
        );
    }

    #[test]
    fn single_attempt_rejected_returns_rejected() {
        // 场景: trace 是 attempt=0, cycles 只有 attempt=0 且 rejected
        // runner 层会据此跳过 (不导出), 但 lookup 本身返回原始结果
        let agent = Uuid::nil();
        let trace = make_trace(agent, 100, 0);
        let mut map = HashMap::new();
        map.insert((agent, 100), metadata(vec![cycle(0, Some("rejected"))]));

        assert_eq!(
            lookup_attempt_match(&trace, &map),
            Some("rejected".to_string())
        );
    }

    #[test]
    fn multi_attempt_no_cross_contamination() {
        // 场景: attempt=0 rejected, attempt=1 approved (Python 的 cycles[-1] 会污染)
        // trace 是 attempt=0 → 应返回 rejected (不串扰 attempt=1 的 approved)
        // trace 是 attempt=1 → 应返回 approved
        let agent = Uuid::nil();
        let trace_0 = make_trace(agent, 100, 0);
        let trace_1 = make_trace(agent, 100, 1);
        let mut map = HashMap::new();
        map.insert(
            (agent, 100),
            metadata(vec![
                cycle(0, Some("rejected")),
                cycle(1, Some("approved")),
            ]),
        );

        // 关键断言: 不取 cycles[-1] (Python 的 bug), 按 attempt 精确匹配
        assert_eq!(
            lookup_attempt_match(&trace_0, &map),
            Some("rejected".to_string()),
            "attempt=0 应返回自身的 rejected, 不串扰 attempt=1 的 approved"
        );
        assert_eq!(
            lookup_attempt_match(&trace_1, &map),
            Some("approved".to_string()),
            "attempt=1 应返回自身的 approved"
        );
    }

    #[test]
    fn attempt_missing_in_cycles_returns_none() {
        // 场景: trace 是 attempt=2, 但 cycles 只有 attempt=0 和 attempt=1
        // 数据不一致 → 返回 None (runner 层会跳过)
        let agent = Uuid::nil();
        let trace = make_trace(agent, 100, 2);
        let mut map = HashMap::new();
        map.insert(
            (agent, 100),
            metadata(vec![
                cycle(0, Some("approved")),
                cycle(1, Some("approved")),
            ]),
        );

        assert_eq!(lookup_attempt_match(&trace, &map), None);
    }

    #[test]
    fn no_audit_record_returns_none() {
        // 场景: trace 的 (agent, tick) 不在 audit_map (tick 还没写完)
        let agent = Uuid::nil();
        let trace = make_trace(agent, 999, 0);
        let map: HashMap<(Uuid, i64), SoulCycleMetadata> = HashMap::new();

        assert_eq!(lookup_attempt_match(&trace, &map), None);
    }

    #[test]
    fn empty_cycles_returns_none() {
        // 场景: metadata 存在但 cycles 为空 (数据不一致)
        let agent = Uuid::nil();
        let trace = make_trace(agent, 100, 0);
        let mut map = HashMap::new();
        map.insert((agent, 100), metadata(vec![]));

        assert_eq!(lookup_attempt_match(&trace, &map), None);
    }

    #[test]
    fn tianhun_result_none_returns_none() {
        // 场景: cycle 存在且 attempt 匹配, 但 tianhun.result 是 None (审查未完成)
        let agent = Uuid::nil();
        let trace = make_trace(agent, 100, 0);
        let mut map = HashMap::new();
        map.insert((agent, 100), metadata(vec![cycle(0, None)]));

        assert_eq!(lookup_attempt_match(&trace, &map), None);
    }
}
