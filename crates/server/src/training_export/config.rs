//! 训练导出配置加载
//!
//! 复刻 main.rs:255-268 的 action_evolution.yaml 加载模式:
//! read_to_string → serde_yaml → .get("data") → serde_json::from_value.
//! env 覆盖用扁平 TRAINING_EXPORT_* (对齐 SERVER_/DB_ 规范, spec §7.2).

use anyhow::{Context, ensure};
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
    /// 等待 bounded manual queue 空位的最长时间。
    pub manual_enqueue_timeout_secs: u64,
    /// manual 请求的排队深度，与 runner 并发数独立。
    pub manual_queue_capacity: usize,
    /// 单日期桶最大 trace_id 数量, None 表示无限。
    pub max_bucket_size: Option<usize>,
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
                manual_enqueue_timeout_secs: 30,
                manual_queue_capacity: 8,
                max_bucket_size: Some(100_000),
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

impl TrainingExportConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.scheduler.interval_secs > 0,
            "training_export.scheduler.interval_secs 必须大于 0"
        );
        ensure!(
            self.scheduler.run_timeout_secs > 0,
            "training_export.scheduler.run_timeout_secs 必须大于 0"
        );
        ensure!(
            self.scheduler.max_concurrent_runs == 1,
            "training_export.scheduler.max_concurrent_runs 当前仅支持 1"
        );
        ensure!(
            self.scheduler.manual_enqueue_timeout_secs > 0,
            "training_export.scheduler.manual_enqueue_timeout_secs 必须大于 0"
        );
        ensure!(
            self.scheduler.manual_queue_capacity > 0,
            "training_export.scheduler.manual_queue_capacity 必须大于 0"
        );
        ensure!(
            self.limits.max_traces_per_run > 0,
            "training_export.limits.max_traces_per_run 必须大于 0"
        );
        ensure!(
            self.limits.max_total_export_size_gb > 0,
            "training_export.limits.max_total_export_size_gb 必须大于 0"
        );
        ensure!(
            self.limits.db_batch_size > 0,
            "training_export.limits.db_batch_size 必须大于 0"
        );
        // spec §5.3: 单次 run DB 查询次数 ≤ 5. 防止运维把 batch 调太小触发几十次查询.
        let db_query_count = self.limits.max_traces_per_run.div_ceil(self.limits.db_batch_size);
        ensure!(
            db_query_count <= 5,
            "training_export: max_traces_per_run ({}) / db_batch_size ({}) = {} > 5, 违反 spec §5.3 单次 run DB 查询次数上限. 请调大 db_batch_size 或调小 max_traces_per_run",
            self.limits.max_traces_per_run,
            self.limits.db_batch_size,
            db_query_count
        );
        ensure!(
            self.limits.db_statement_timeout_secs > 0,
            "training_export.limits.db_statement_timeout_secs 必须大于 0"
        );
        ensure!(
            self.limits.yield_every_n > 0,
            "training_export.limits.yield_every_n 必须大于 0"
        );
        ensure!(
            self.checkpoint.retain_days >= 0,
            "training_export.checkpoint.retain_days 不能小于 0"
        );
        validate_relative_path(
            "training_export.paths.traces_input_subdir",
            &self.paths.traces_input_subdir,
        )?;
        validate_relative_path(
            "training_export.paths.output_subdir",
            &self.paths.output_subdir,
        )?;
        validate_relative_path(
            "training_export.paths.checkpoint_filename",
            &self.paths.checkpoint_filename,
        )?;
        Ok(())
    }
}

fn validate_relative_path(name: &str, value: &str) -> anyhow::Result<()> {
    let path = std::path::Path::new(value);
    ensure!(!value.is_empty(), "{name} 不能为空");
    ensure!(!path.is_absolute(), "{name} 必须是相对路径");
    ensure!(
        path.components()
            .all(|component| matches!(component, std::path::Component::Normal(_))),
        "{name} 不能包含 .、..、根目录或平台前缀"
    );
    Ok(())
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
            ));
        }
    };

    apply_env_overrides(&mut cfg)?;
    cfg.validate()?;
    Ok(cfg)
}

fn apply_env_overrides(cfg: &mut TrainingExportConfig) -> anyhow::Result<()> {
    if let Some(value) = env_string("TRAINING_EXPORT_ENABLED")? {
        cfg.enabled = match value.to_lowercase().as_str() {
            "true" | "1" => true,
            "false" | "0" => false,
            _ => anyhow::bail!("TRAINING_EXPORT_ENABLED 必须是 true/false/1/0, 实际为 {value:?}"),
        };
    }
    if let Some(value) = env_u64("TRAINING_EXPORT_INTERVAL_SECS")? {
        cfg.scheduler.interval_secs = value;
    }
    if let Some(value) = env_u64("TRAINING_EXPORT_RUN_TIMEOUT_SECS")? {
        cfg.scheduler.run_timeout_secs = value;
    }
    if let Some(value) = env_u64("TRAINING_EXPORT_MANUAL_ENQUEUE_TIMEOUT_SECS")? {
        cfg.scheduler.manual_enqueue_timeout_secs = value;
    }
    if let Some(value) = env_usize("TRAINING_EXPORT_MANUAL_QUEUE_CAPACITY")? {
        cfg.scheduler.manual_queue_capacity = value;
    }
    if let Some(value) = env_usize("TRAINING_EXPORT_MAX_TRACES")? {
        cfg.limits.max_traces_per_run = value;
    }
    if let Some(value) = env_u64("TRAINING_EXPORT_MAX_SIZE_GB")? {
        cfg.limits.max_total_export_size_gb = value;
    }
    if let Some(value) = env_usize("TRAINING_EXPORT_DB_BATCH")? {
        cfg.limits.db_batch_size = value;
    }
    if let Some(value) = env_u64("TRAINING_EXPORT_DB_STATEMENT_TIMEOUT_SECS")? {
        cfg.limits.db_statement_timeout_secs = value;
    }
    if let Some(value) = env_usize("TRAINING_EXPORT_YIELD_EVERY_N")? {
        cfg.limits.yield_every_n = value;
    }
    if let Some(value) = env_i64("TRAINING_EXPORT_RETAIN_DAYS")? {
        cfg.checkpoint.retain_days = value;
    }
    if let Some(value) = env_string("TRAINING_EXPORT_TRACES_INPUT_SUBDIR")? {
        cfg.paths.traces_input_subdir = value;
    }
    if let Some(value) = env_string("TRAINING_EXPORT_OUTPUT_SUBDIR")? {
        cfg.paths.output_subdir = value;
    }
    if let Some(value) = env_string("TRAINING_EXPORT_CHECKPOINT_FILENAME")? {
        cfg.paths.checkpoint_filename = value;
    }
    if let Some(value) = env_usize("TRAINING_EXPORT_MAX_BUCKET_SIZE")? {
        cfg.scheduler.max_bucket_size = if value == 0 { None } else { Some(value) };
    }
    Ok(())
}

fn env_string(name: &str) -> anyhow::Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error).with_context(|| format!("读取环境变量 {name} 失败")),
    }
}

fn env_u64(name: &str) -> anyhow::Result<Option<u64>> {
    env_string(name)?
        .map(|value| {
            value
                .parse::<u64>()
                .with_context(|| format!("环境变量 {name} 必须是 u64, 实际为 {value:?}"))
        })
        .transpose()
}

fn env_usize(name: &str) -> anyhow::Result<Option<usize>> {
    env_string(name)?
        .map(|value| {
            value
                .parse::<usize>()
                .with_context(|| format!("环境变量 {name} 必须是 usize, 实际为 {value:?}"))
        })
        .transpose()
}

fn env_i64(name: &str) -> anyhow::Result<Option<i64>> {
    env_string(name)?
        .map(|value| {
            value
                .parse::<i64>()
                .with_context(|| format!("环境变量 {name} 必须是 i64, 实际为 {value:?}"))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::TrainingExportConfig;

    #[test]
    fn default_config_is_valid() {
        assert!(TrainingExportConfig::default().validate().is_ok());
    }

    #[test]
    fn zero_manual_timeout_is_rejected() {
        let mut config = TrainingExportConfig::default();
        config.scheduler.manual_enqueue_timeout_secs = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn repository_yaml_has_manual_queue_settings() {
        let config_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config");
        let config = super::load_config(&config_dir).expect("load training_export.yaml");
        assert_eq!(config.scheduler.manual_enqueue_timeout_secs, 30);
        assert_eq!(config.scheduler.manual_queue_capacity, 8);
    }

    #[test]
    fn output_path_traversal_is_rejected() {
        let mut config = TrainingExportConfig::default();
        config.paths.output_subdir = "../outside".to_string();
        assert!(config.validate().is_err());
    }
}
