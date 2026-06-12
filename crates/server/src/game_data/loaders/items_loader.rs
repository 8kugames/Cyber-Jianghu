// ============================================================================
// OpenClaw Cyber-Jianghu 物品配置加载器
// ============================================================================
//
// 本模块负责加载物品配置（items.yaml 或 items.json）
// ============================================================================

use crate::game_data::loaders::config_format::load_config;
use crate::game_data::types::UnifiedItemsConfig;
use anyhow::{Context, Result};
use std::path::Path;

/// 加载物品配置
///
/// 优先加载 YAML 格式，回退到 JSON 格式。
///
/// # 参数
/// - `config_dir`: 配置文件目录路径
///
/// # 返回
/// 统一物品配置对象
pub fn load_items<P: AsRef<Path>>(config_dir: P) -> Result<UnifiedItemsConfig> {
    let config_dir = config_dir.as_ref();

    // 优先尝试 YAML 格式
    let yaml_path = config_dir.join("items.yaml");
    if yaml_path.exists() {
        return load_config(&yaml_path).context("加载物品配置 (YAML) 失败");
    }

    // 回退到 JSON 格式
    let json_path = config_dir.join("items.json");
    load_config(&json_path).context("加载物品配置 (JSON) 失败")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_data::loaders::config_format::{ConfigFormat, parse_config};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_items_json() {
        let dir = TempDir::new().unwrap();

        // 创建测试配置文件
        fs::write(
            dir.path().join("items.json"),
            r#"{
                "version": "2.0.0",
                "description": "物品配置文件",
                "meta": {},
                "data": [
                    {
                        "item_id": "馒头",
                        "name": "馒头",
                        "item_type": "consumable",
                        "effects": [
                            {
                                "attribute": "satiation",
                                "operation": "add",
                                "value": 30
                            }
                        ],
                        "stack_size": 10,
                        "description": "热腾腾的馒头"
                    }
                ]
            }"#,
        )
        .unwrap();

        let items = load_items(dir.path()).unwrap();
        assert_eq!(items.version, "2.0.0");
        assert_eq!(items.data.len(), 1);
        assert_eq!(items.data[0].item_id, "馒头");
    }

    #[test]
    fn test_load_items_yaml() {
        let yaml = r#"
version: "2.0.0"
description: "物品配置文件"
meta: {}
data:
  - item_id: "馒头"
    name: "馒头"
    item_type: "consumable"
    effects:
      - attribute: "satiation"
        operation: "add"
        value: 30
    stack_size: 10
    description: "热腾腾的馒头"
"#;

        let config: UnifiedItemsConfig = parse_config(yaml, ConfigFormat::Yaml).unwrap();
        assert_eq!(config.version, "2.0.0");
        assert_eq!(config.data.len(), 1);
        assert_eq!(config.data[0].item_id, "馒头");
    }
}
