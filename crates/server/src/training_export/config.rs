//! 训练导出配置加载
//!
//! 复刻 main.rs:255-268 的 action_evolution.yaml 加载模式:
//! read_to_string → serde_yaml → .get("data") → serde_json::from_value.
//! env 覆盖用扁平 TRAINING_EXPORT_* (对齐 SERVER_/DB_ 规范, spec §7.2).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingExportConfig {
    pub enabled: bool,
    pub scheduler: SchedulerConfig,
    pub limits: LimitsConfig,
    pub checkpoint: CheckpointConfig,
    pub paths: PathsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    pub interval_secs: u64,
    pub run_timeout_secs: u64,
    pub max_concurrent_runs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsConfig {
    pub max_traces_per_run: usize,
    pub max_total_export_size_gb: u64,
    pub db_batch_size: usize,
    pub db_statement_timeout_secs: u64,
    pub yield_every_n: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointConfig {
    pub retain_days: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathsConfig {
    pub traces_input_subdir: String,
    pub output_subdir: String,
    pub checkpoint_filename: String,
}

impl Default for TrainingExportConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            scheduler: SchedulerConfig {
                interval_secs: 21600,
                run_timeout_secs: 600,
                max_concurrent_runs: 1,
            },
            limits: LimitsConfig {
                max_traces_per_run: 50000,
                max_total_export_size_gb: 50,
                db_batch_size: 10000,
                db_statement_timeout_secs: 30,
                yield_every_n: 500,
            },
            checkpoint: CheckpointConfig { retain_days: 7 },
            paths: PathsConfig {
                traces_input_subdir: "traces/soul=renhun".to_string(),
                output_subdir: "training_exports/sft".to_string(),
                checkpoint_filename: "training_exports/sft_checkpoint.json".to_string(),
            },
        }
    }
}

pub fn load_config(config_dir: &std::path::Path) -> anyhow::Result<TrainingExportConfig> {
    let path = config_dir.join("training_export.yaml");
    let mut cfg = match std::fs::read_to_string(&path) {
        Ok(content) => {
            let outer: serde_json::Value = serde_yaml::from_str(&content)
                .with_context(|| format!("解析 training_export.yaml 失败: {:?}", path))?;
            let data = outer
                .get("data")
                .context("training_export.yaml 缺少 data 字段")?;
            serde_json::from_value(data.clone())
                .with_context(|| "反序列化 TrainingExportConfig 失败")?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!("training_export.yaml 不存在, 使用 default 配置");
            TrainingExportConfig::default()
        }
        Err(e) => {
            return Err(anyhow::anyhow!(
                "读取 training_export.yaml 失败 {:?}: {}",
                path,
                e
            ))
        }
    };

    apply_env_overrides(&mut cfg);
    Ok(cfg)
}

fn apply_env_overrides(cfg: &mut TrainingExportConfig) {
    if let Ok(v) = std::env::var("TRAINING_EXPORT_ENABLED") {
        cfg.enabled = matches!(v.to_lowercase().as_str(), "true" | "1");
    }
    if let Some(v) = env_u64("TRAINING_EXPORT_INTERVAL_SECS") {
        cfg.scheduler.interval_secs = v;
    }
    if let Some(v) = env_u64("TRAINING_EXPORT_RUN_TIMEOUT_SECS") {
        cfg.scheduler.run_timeout_secs = v;
    }
    if let Some(v) = env_usize("TRAINING_EXPORT_MAX_TRACES") {
        cfg.limits.max_traces_per_run = v;
    }
    if let Some(v) = env_u64("TRAINING_EXPORT_MAX_SIZE_GB") {
        cfg.limits.max_total_export_size_gb = v;
    }
    if let Some(v) = env_usize("TRAINING_EXPORT_DB_BATCH") {
        cfg.limits.db_batch_size = v;
    }
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}
fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

use anyhow::Context as _;
