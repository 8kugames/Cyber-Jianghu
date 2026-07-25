//! 单次 run 编排 (Task 7 完整实现, 本 Task 先放 db 查询)

use std::collections::HashMap;

use cyber_jianghu_protocol::SoulCycleMetadata;
use sqlx::PgPool;
use uuid::Uuid;

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

    let agent_ids: Vec<Uuid> = keys.iter().map(|(a, _)| *a).collect();
    let tick_ids: Vec<i64> = keys.iter().map(|(_, t)| *t).collect();

    let mut tx = pool.begin().await?;
    sqlx::query(&format!(
        "SET LOCAL statement_timeout = '{}s'",
        statement_timeout_secs
    ))
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
                Ok(m) => {
                    map.insert((row.agent_id, row.tick_id), m);
                }
                Err(e) => {
                    tracing::warn!(
                        agent_id = %row.agent_id,
                        tick_id = row.tick_id,
                        "解析 soul_cycle_metadata 失败: {}",
                        e
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

use anyhow::Context as _;

use crate::training_export::checkpoint::Checkpoint;
use crate::training_export::config::TrainingExportConfig;
use crate::training_export::sft_transform::{transform_entry, SftSample, TransformInput};
use crate::training_export::{RunMetadata, RunStatus, TriggerSource};

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
    triggered_by: TriggerSource,
    run_id: String,
) -> anyhow::Result<RunResult> {
    let data_dir = crate::paths::get_data_dir();
    let traces_dir = data_dir.join(&config.paths.traces_input_subdir);
    let output_dir = data_dir.join(&config.paths.output_subdir);

    let mut metadata = RunMetadata::new_pending(run_id.clone(), triggered_by);
    metadata.status = RunStatus::Running;

    // Step 1: 扫描 trace 文件
    let (entries, keys, trace_dates) =
        scan_trace_files(&traces_dir, config.limits.max_traces_per_run).await?;
    metadata.trace_count = entries.len();

    if entries.is_empty() {
        metadata.status = RunStatus::Completed;
        metadata.completed_at = Some(chrono::Utc::now().timestamp_millis());
        return Ok(RunResult {
            metadata,
            samples: vec![],
        });
    }

    // Step 2: 批量查 DB
    let audit_map = fetch_audit_map_batched(
        pool,
        &keys,
        config.limits.db_batch_size,
        config.limits.db_statement_timeout_secs,
    )
    .await?;

    // Step 3 + 4: filter (ok + attempt 匹配 approved) + transform
    let mut samples: Vec<SftSample> = Vec::new();
    let yield_every = config.limits.yield_every_n.max(1);
    for (i, (entry, date)) in entries.iter().zip(trace_dates.iter()).enumerate() {
        if checkpoint.is_processed(date, &entry.trace_id) {
            continue;
        }
        let tianhun_result = lookup_attemp_match(entry, &audit_map);
        let should_export = matches!(tianhun_result.as_deref(), Some("approved"));
        if should_export {
            if let Some(sample) = transform_entry(TransformInput {
                entry,
                tianhun_result: tianhun_result.clone(),
            }) {
                samples.push(sample);
                checkpoint.mark_processed(date, entry.trace_id.clone());
            }
        }
        if i % yield_every == 0 {
            tokio::task::yield_now().await;
        }
    }

    metadata.sample_count = samples.len();

    // Step 5: 写产物 (.tmp + rename + fsync, spec §8.3)
    tokio::fs::create_dir_all(&output_dir).await?;
    let output_path = output_dir.join(format!("run={}.jsonl", run_id));
    let tmp_path = output_path.with_extension("jsonl.tmp");
    let mut content = String::new();
    for s in &samples {
        content.push_str(&serde_json::to_string(s)?);
        content.push('\n');
    }
    tokio::fs::write(&tmp_path, &content).await?;
    {
        let f = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&tmp_path)
            .await?;
        f.sync_all().await?;
    }
    tokio::fs::rename(&tmp_path, &output_path).await?;

    // 写 .meta.json
    metadata.output_path = output_path
        .strip_prefix(&data_dir)
        .unwrap_or(&output_path)
        .to_string_lossy()
        .to_string();
    metadata.output_size_bytes = content.len() as u64;
    metadata.status = RunStatus::Completed;
    metadata.completed_at = Some(chrono::Utc::now().timestamp_millis());

    let meta_path = output_dir.join(format!("run={}.meta.json", run_id));
    let meta_tmp = meta_path.with_extension("json.tmp");
    tokio::fs::write(&meta_tmp, serde_json::to_string_pretty(&metadata)?).await?;
    tokio::fs::rename(&meta_tmp, &meta_path).await?;

    Ok(RunResult { metadata, samples })
}

async fn scan_trace_files(
    traces_dir: &std::path::Path,
    max_traces: usize,
) -> anyhow::Result<(
    Vec<cyber_jianghu_protocol::TraceEntry>,
    Vec<(Uuid, i64)>,
    Vec<String>,
)> {
    let mut entries = Vec::new();
    let mut keys = std::collections::HashSet::new();
    let mut dates = Vec::new();

    if !traces_dir.exists() {
        return Ok((entries, keys.into_iter().collect(), dates));
    }

    let mut agent_dirs = tokio::fs::read_dir(traces_dir).await?;
    while let Ok(Some(agent_entry)) = agent_dirs.next_entry().await {
        if !agent_entry
            .file_type()
            .await
            .map(|t| t.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        let mut date_files = tokio::fs::read_dir(agent_entry.path()).await?;
        while let Ok(Some(date_entry)) = date_files.next_entry().await {
            let path = date_entry.path();
            if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                let date_str = path
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .and_then(|s| s.strip_prefix("date="))
                    .unwrap_or("unknown")
                    .to_string();

                let content = tokio::fs::read_to_string(&path).await?;
                for line in content.lines() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    if entries.len() >= max_traces {
                        break;
                    }
                    match serde_json::from_str::<cyber_jianghu_protocol::TraceEntry>(line) {
                        Ok(entry) => {
                            keys.insert((entry.agent_id, entry.tick_id));
                            entries.push(entry);
                            dates.push(date_str.clone());
                        }
                        Err(e) => {
                            tracing::warn!("解析 trace 行失败 {:?}: {}", path, e);
                        }
                    }
                }
                if entries.len() >= max_traces {
                    break;
                }
            }
        }
        if entries.len() >= max_traces {
            break;
        }
    }

    Ok((entries, keys.into_iter().collect(), dates))
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

fn lookup_attemp_match(
    entry: &cyber_jianghu_protocol::TraceEntry,
    audit_map: &HashMap<(Uuid, i64), SoulCycleMetadata>,
) -> Option<String> {
    let metadata = audit_map.get(&(entry.agent_id, entry.tick_id))?;
    let cycle = metadata.cycles.iter().find(|c| c.attempt == entry.attempt)?;
    cycle.tianhun.result.clone()
}
