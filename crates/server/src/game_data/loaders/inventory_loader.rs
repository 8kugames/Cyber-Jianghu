// ============================================================================
// OpenClaw Cyber-Jianghu 背包配置加载器
// ============================================================================
//
// 本模块负责加载背包相关配置：
// - initial_inventory.yaml/json (初始物品清单)
// - inventory.yaml/json (背包限制)
// ============================================================================

use crate::game_data::loaders::config_format::load_config;
use crate::game_data::types::{UnifiedInitialInventoryConfig, UnifiedInventoryConfig};
use anyhow::{Context, Result};
use std::path::Path;

/// 加载初始物品配置
///
/// 优先加载 YAML 格式，回退到 JSON 格式。
///
/// # 参数
/// - `config_dir`: 配置文件目录路径
///
/// # 返回
/// 统一初始物品配置对象
pub fn load_initial_inventory<P: AsRef<Path>>(
    config_dir: P,
) -> Result<UnifiedInitialInventoryConfig> {
    let config_dir = config_dir.as_ref();

    // 优先尝试 YAML 格式
    let yaml_path = config_dir.join("initial_inventory.yaml");
    if yaml_path.exists() {
        return load_config(&yaml_path).context("加载初始物品配置 (YAML) 失败");
    }

    // 回退到 JSON 格式
    let json_path = config_dir.join("initial_inventory.json");
    load_config(&json_path).context("加载初始物品配置 (JSON) 失败")
}

/// 加载背包配置
///
/// 优先加载 YAML 格式，回退到 JSON 格式。
///
/// # 参数
/// - `config_dir`: 配置文件目录路径
///
/// # 返回
/// 统一背包配置对象
pub fn load_inventory<P: AsRef<Path>>(config_dir: P) -> Result<UnifiedInventoryConfig> {
    let config_dir = config_dir.as_ref();

    // 优先尝试 YAML 格式
    let yaml_path = config_dir.join("inventory.yaml");
    if yaml_path.exists() {
        return load_config(&yaml_path).context("加载背包配置 (YAML) 失败");
    }

    // 回退到 JSON 格式
    let json_path = config_dir.join("inventory.json");
    load_config(&json_path).context("加载背包配置 (JSON) 失败")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_data::loaders::config_format::{ConfigFormat, parse_config};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_initial_inventory_json() {
        let dir = TempDir::new().unwrap();

        // 创建测试配置文件
        fs::write(
            dir.path().join("initial_inventory.json"),
            r#"{
                "version": "2.0.0",
                "description": "初始物品配置",
                "meta": {},
                "data": {
                    "items": {
                        "food": [
                            { "item_id": "馒头", "name": "馒头", "quantity": 3, "description": "热腾腾的馒头" }
                        ]
                    }
                }
            }"#,
        ).unwrap();

        let config = load_initial_inventory(dir.path()).unwrap();
        assert_eq!(config.version, "2.0.0");
        assert_eq!(config.data.items.len(), 1);
        assert_eq!(config.data.items[0].item_id, "馒头");
        assert_eq!(config.data.items[0].quantity, 3);
    }

    #[test]
    fn test_load_initial_inventory_yaml() {
        let yaml = r#"
version: "2.0.0"
description: "初始物品配置"
meta: {}
data:
  items:
    food:
      - item_id: "馒头"
        name: "馒头"
        quantity: 3
        description: "热腾腾的馒头"
"#;

        let config: UnifiedInitialInventoryConfig = parse_config(yaml, ConfigFormat::Yaml).unwrap();
        assert_eq!(config.version, "2.0.0");
        assert_eq!(config.data.items.len(), 1);
        assert_eq!(config.data.items[0].item_id, "馒头");
    }

    #[test]
    fn test_load_initial_inventory_yaml_flat_items_should_fail() {
        let yaml = r#"
version: "2.0.0"
description: "初始物品配置"
meta: {}
data:
  items:
    - item_id: "馒头"
      name: "馒头"
      quantity: 3
      description: "热腾腾的馒头"
"#;

        let result: Result<UnifiedInitialInventoryConfig, _> =
            parse_config(yaml, ConfigFormat::Yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_inventory_json() {
        let dir = TempDir::new().unwrap();

        // 创建测试配置文件
        fs::write(
            dir.path().join("inventory.json"),
            r#"{
                "version": "2.0.0",
                "description": "背包配置",
                "meta": {},
                "data": {
                    "max_slots": 10,
                    "max_stack_size": 10
                }
            }"#,
        )
        .unwrap();

        let config = load_inventory(dir.path()).unwrap();
        assert_eq!(config.version, "2.0.0");
        assert_eq!(config.data.max_slots, 10);
        assert_eq!(config.data.max_stack_size, 10);
    }

    #[test]
    fn test_load_inventory_yaml() {
        let yaml = r#"
version: "2.0.0"
description: "背包配置"
meta: {}
data:
  max_slots: 10
  max_stack_size: 10
"#;

        let config: UnifiedInventoryConfig = parse_config(yaml, ConfigFormat::Yaml).unwrap();
        assert_eq!(config.version, "2.0.0");
        assert_eq!(config.data.max_slots, 10);
        assert_eq!(config.data.max_stack_size, 10);
    }
}
