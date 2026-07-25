//! Checkpoint 读写 - trace_id 集合幂等去重
//!
//! 设计 spec §5.2/§5.2.1: 按 date=YYYY-MM-DD 分桶记录已处理 trace_id,
//! 超过 N 天 (默认 7) 的桶整桶删除, 防 checkpoint 无限膨胀.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 单个日期桶: 该日期所有已处理 trace_id
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DateBucket {
    pub trace_ids: std::collections::HashSet<String>,
}

/// Checkpoint 全量状态 (序列化为 sft_checkpoint.json)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Checkpoint {
    /// schema 版本
    pub version: u32,
    /// 上次 run 的 Unix ms
    pub last_run_at: Option<i64>,
    /// key: "YYYY-MM-DD" (trace 文件日期分区)
    pub buckets: HashMap<String, DateBucket>,
}

impl Checkpoint {
    /// 从文件加载; 不存在返回空 Checkpoint
    pub async fn load(path: &Path) -> anyhow::Result<Self> {
        match tokio::fs::read_to_string(path).await {
            Ok(content) => {
                let cp: Self = serde_json::from_str(&content)
                    .with_context(|| format!("解析 checkpoint 失败: {:?}", path))?;
                Ok(cp)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(anyhow::anyhow!("读取 checkpoint 失败 {:?}: {}", path, e)),
        }
    }

    /// 原子写: .tmp + rename (spec §8.3)
    pub async fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp = path.with_extension("json.tmp");
        let content = serde_json::to_string_pretty(self)?;
        tokio::fs::write(&tmp, content).await?;
        tokio::fs::rename(&tmp, path)
            .await
            .with_context(|| format!("checkpoint rename 失败: {:?}", path))?;
        Ok(())
    }

    /// 该 trace_id 是否已处理 (任意桶)
    pub fn is_processed(&self, date: &str, trace_id: &str) -> bool {
        self.buckets
            .get(date)
            .map(|b| b.trace_ids.contains(trace_id))
            .unwrap_or(false)
    }

    /// 标记 trace_id 已处理 (幂等)
    pub fn mark_processed(&mut self, date: &str, trace_id: String) {
        self.buckets
            .entry(date.to_string())
            .or_default()
            .trace_ids
            .insert(trace_id);
    }

    /// 退役超过 retain_days 天的桶 (spec §5.2.1)
    /// date 格式 "YYYY-MM-DD"; now_date 是当前日期 (UTC)
    pub fn retire_old_buckets(&mut self, retain_days: i64, now_date: &str) {
        let now = parse_date(now_date);
        self.buckets.retain(|date_str, _bucket| {
            match parse_date(date_str) {
                Some(d) => {
                    let age_days = (now.unwrap_or(d) - d).num_days();
                    age_days <= retain_days
                }
                None => true, // 无法解析的日期保留 (保守)
            }
        });
    }
}

fn parse_date(s: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// 扫描目录删除残留 .tmp 文件 (spec §8.3 启动 sweep)
pub async fn sweep_tmp_files(dir: &Path) -> anyhow::Result<usize> {
    let mut count = 0;
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(anyhow::anyhow!("读取目录 {:?} 失败: {}", dir, e)),
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().map(|e| e == "tmp").unwrap_or(false) {
            if let Err(e) = tokio::fs::remove_file(&path).await {
                tracing::warn!("sweep 删除 {:?} 失败: {}", path, e);
            } else {
                count += 1;
            }
        }
    }
    Ok(count)
}

// 供 anyhow context 用
use anyhow::Context as _;

/// checkpoint 文件路径 (相对 data_dir)
pub fn checkpoint_path(data_dir: &Path) -> PathBuf {
    data_dir.join("training_exports").join("sft_checkpoint.json")
}
