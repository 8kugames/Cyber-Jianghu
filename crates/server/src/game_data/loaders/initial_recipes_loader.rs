// ============================================================================
// OpenClaw Cyber-Jianghu 初始配方配置加载器
// ============================================================================

use crate::game_data::loaders::config_format::load_config;
use crate::game_data::types::UnifiedInitialRecipesConfig;
use anyhow::{Context, Result};
use std::path::Path;

/// 加载初始配方配置
///
/// 优先加载 YAML 格式，回退到 JSON 格式。
pub fn load_initial_recipes<P: AsRef<Path>>(
    config_dir: P,
) -> Result<UnifiedInitialRecipesConfig> {
    let config_dir = config_dir.as_ref();

    let yaml_path = config_dir.join("initial_recipes.yaml");
    if yaml_path.exists() {
        return load_config(&yaml_path).context("加载初始配方配置 (YAML) 失败");
    }

    let json_path = config_dir.join("initial_recipes.json");
    load_config(&json_path).context("加载初始配方配置 (JSON) 失败")
}
