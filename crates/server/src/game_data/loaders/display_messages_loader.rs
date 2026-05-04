// ============================================================================
// 显示消息配置加载器
// ============================================================================
//
// 从 display_messages.yaml 加载显示消息配置

use crate::game_data::loaders::config_format::ConfigFormat;
use crate::game_data::types::DisplayMessagesConfig;
use anyhow::{Context, Result};
use std::path::Path;

/// 加载显示消息配置
///
/// 优先加载 YAML 格式，回退到 JSON 格式
pub fn load_display_messages(config_dir: &Path) -> Result<DisplayMessagesConfig> {
    // 优先尝试 YAML 格式
    let yaml_path = config_dir.join("display_messages.yaml");
    if yaml_path.exists() {
        return load_from_path(&yaml_path, ConfigFormat::Yaml);
    }

    // 回退到 JSON 格式
    let json_path = config_dir.join("display_messages.json");
    if json_path.exists() {
        return load_from_path(&json_path, ConfigFormat::Json);
    }

    Err(anyhow::anyhow!(
        "[display_messages_loader] 显示消息配置文件不存在: {:?} 或 {:?}",
        yaml_path,
        json_path
    ))
}

fn load_from_path(path: &Path, format: ConfigFormat) -> Result<DisplayMessagesConfig> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read display messages config from {:?}", path))?;

    match crate::game_data::loaders::config_format::parse_config(&content, format) {
        Ok(config) => {
            tracing::info!(
                "[display_messages_loader] Loaded display messages config from {}",
                path.display()
            );
            Ok(config)
        }
        Err(e) => Err(anyhow::anyhow!(
            "[display_messages_loader] 解析配置文件失败: {}，路径: {:?}",
            e,
            path
        )),
    }
}
