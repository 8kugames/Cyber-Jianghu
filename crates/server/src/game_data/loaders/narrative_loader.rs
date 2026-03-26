// ============================================================================
// 叙事化配置加载器
// ============================================================================
//
// 从 narrative_config.yaml 或 narrative_config.json 加载叙事化配置

use crate::game_data::loaders::config_format::ConfigFormat;
use anyhow::{Context, Result};
use cyber_jianghu_protocol::NarrativeConfig;
use std::path::Path;

/// 加载叙事化配置
///
/// 优先加载 YAML 格式，回退到 JSON 格式。
///
/// # 参数
/// - `config_dir`: 配置文件目录
///
/// # 返回
/// 叙事化配置，如果文件不存在则返回默认配置
pub fn load_narrative(config_dir: &Path) -> Result<NarrativeConfig> {
    // 优先尝试 YAML 格式
    let yaml_path = config_dir.join("narrative_config.yaml");
    if yaml_path.exists() {
        return load_narrative_from_path(&yaml_path, ConfigFormat::Yaml);
    }

    // 回退到 JSON 格式
    let json_path = config_dir.join("narrative_config.json");
    if json_path.exists() {
        return load_narrative_from_path(&json_path, ConfigFormat::Json);
    }

    tracing::info!("[narrative_loader] Config file not found, using builtin config");
    Ok(NarrativeConfig::builtin())
}

fn load_narrative_from_path(path: &Path, format: ConfigFormat) -> Result<NarrativeConfig> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read narrative config from {:?}", path))?;

    // 尝试解析为带 data 包装的格式
    #[derive(serde::Deserialize)]
    struct WrappedConfig {
        data: Option<NarrativeConfig>,
    }

    // 首先尝试解析带包装的格式
    if let Ok(wrapped) =
        crate::game_data::loaders::config_format::parse_config::<WrappedConfig>(&content, format)
        && let Some(data) = wrapped.data
    {
        tracing::info!(
            "[narrative_loader] Loaded narrative config from {}",
            path.display()
        );
        return Ok(data);
    }

    // 尝试直接解析为 NarrativeConfig
    match crate::game_data::loaders::config_format::parse_config(&content, format) {
        Ok(config) => {
            tracing::info!(
                "[narrative_loader] Loaded narrative config from {}",
                path.display()
            );
            Ok(config)
        }
        Err(e) => {
            tracing::warn!(
                "[narrative_loader] Failed to parse narrative config: {}, using builtin",
                e
            );
            Ok(NarrativeConfig::builtin())
        }
    }
}
