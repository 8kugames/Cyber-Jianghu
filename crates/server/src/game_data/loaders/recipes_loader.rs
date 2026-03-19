use anyhow::{Context, Result};
use std::path::Path;

use crate::game_data::loaders::config_format::load_config;
use crate::game_data::types::UnifiedRecipesConfig;

/// 加载配方配置
///
/// 从指定的配置文件加载配方配置数据。
/// 支持 JSON (.json) 和 YAML (.yaml/.yml) 格式。
///
/// # 参数
/// * `path` - 配置文件路径
///
/// # 返回
/// * `Result<UnifiedRecipesConfig>` - 加载成功返回配置对象，失败返回错误
pub fn load_recipes<P: AsRef<Path>>(path: P) -> Result<UnifiedRecipesConfig> {
    load_config(path).context("加载配方配置失败")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_data::loaders::config_format::{ConfigFormat, parse_config};

    #[test]
    fn test_load_recipes_json() {
        let json = r#"{
            "version": "2.0.0",
            "description": "配方配置文件",
            "meta": {},
            "data": {
                "test_recipe": {
                    "name": "测试配方",
                    "description": "测试描述",
                    "result_item": "test_item",
                    "result_quantity": 1,
                    "materials": [],
                    "tools": [],
                    "stamina_cost": 5
                }
            }
        }"#;

        let config: UnifiedRecipesConfig = parse_config(json, ConfigFormat::Json).unwrap();
        assert_eq!(config.version, "2.0.0");
        assert!(config.data.contains_key("test_recipe"));
    }

    #[test]
    fn test_load_recipes_yaml() {
        let yaml = r#"
version: "2.0.0"
description: "配方配置文件"
meta: {}
data:
  test_recipe:
    name: "测试配方"
    description: "测试描述"
    result_item: "test_item"
    result_quantity: 1
    materials: []
    tools: []
    stamina_cost: 5
"#;

        let config: UnifiedRecipesConfig = parse_config(yaml, ConfigFormat::Yaml).unwrap();
        assert_eq!(config.version, "2.0.0");
        assert!(config.data.contains_key("test_recipe"));
    }
}
