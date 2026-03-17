// ============================================================================
// 叙事化配置加载器
// ============================================================================
//
// 从 narrative_config.json 加载叙事化配置

use anyhow::{Context, Result};
use cyber_jianghu_protocol::NarrativeConfig;
use std::path::Path;

/// 加载叙事化配置
///
/// # 参数
/// - `config_dir`: 配置文件目录
///
/// # 返回
/// 叙事化配置，如果文件不存在则返回默认配置
pub fn load_narrative(config_dir: &Path) -> Result<NarrativeConfig> {
    let config_path = config_dir.join("narrative_config.json");

    if !config_path.exists() {
        tracing::info!("[narrative_loader] Config file not found, using builtin config");
        return Ok(NarrativeConfig::builtin());
    }

    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read narrative config from {:?}", config_path))?;

    // 尝试解析为带 data 包装的格式
    #[derive(serde::Deserialize)]
    struct WrappedConfig {
        data: Option<NarrativeConfig>,
    }

    // 首先尝试解析带包装的格式
    if let Ok(wrapped) = serde_json::from_str::<WrappedConfig>(&content) {
        if let Some(data) = wrapped.data {
            tracing::info!("[narrative_loader] Loaded narrative config from {}", config_path.display());
            return Ok(data);
        }
    }

    // 尝试直接解析为 NarrativeConfig
    match serde_json::from_str(&content) {
        Ok(config) => {
            tracing::info!("[narrative_loader] Loaded narrative config from {}", config_path.display());
            Ok(config)
        }
        Err(e) => {
            tracing::warn!("[narrative_loader] Failed to parse narrative config: {}, using builtin", e);
            Ok(NarrativeConfig::builtin())
        }
    }
}
