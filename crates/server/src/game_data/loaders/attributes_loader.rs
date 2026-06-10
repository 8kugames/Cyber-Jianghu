// ============================================================================
// OpenClaw Cyber-Jianghu 属性配置加载器
// ============================================================================
//
// 本模块负责加载统一属性配置 (attributes.yaml 或 attributes.json)
// ============================================================================

use crate::game_data::loaders::config_format::load_config;
use crate::game_data::types::UnifiedAttributesConfig;
use anyhow::{Context, Result};
use std::path::Path;

/// 加载统一属性配置
///
/// 优先加载 YAML 格式，回退到 JSON 格式。
///
/// # 参数
/// - `config_dir`: 配置文件目录路径
///
/// # 返回
/// 统一属性配置对象
pub fn load_unified_attributes<P: AsRef<Path>>(config_dir: P) -> Result<UnifiedAttributesConfig> {
    let config_dir = config_dir.as_ref();

    // 优先尝试 YAML 格式
    let yaml_path = config_dir.join("attributes.yaml");
    if yaml_path.exists() {
        return load_config(&yaml_path).context("加载属性配置 (YAML) 失败");
    }

    // 回退到 JSON 格式
    let json_path = config_dir.join("attributes.json");
    load_config(&json_path).context("加载属性配置 (JSON) 失败")
}

/// 加载统一属性配置（便捷别名）
///
/// # 参数
/// - `config_dir`: 配置文件目录路径
///
/// # 返回
/// 统一属性配置对象
pub fn load_attributes<P: AsRef<Path>>(config_dir: P) -> Result<UnifiedAttributesConfig> {
    load_unified_attributes(config_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_unified_config(dir: &TempDir) {
        // 创建 attributes.json (统一属性配置)
        fs::write(
            dir.path().join("attributes.json"),
            r#"{
                "version": "0.0.1",
                "description": "统一属性配置文件（数据驱动架构 - 统一数据格式）",
                "meta": {
                    "created_at": "2026-03-16",
                    "author": "System"
                },
                "data": {
                    "primary": {
                        "description": "先天属性（DND风格）",
                        "attributes": {
                            "strength": {
                                "name": "strength",
                                "type": "static",
                                "display_name": "力量",
                                "description": "影响物理攻击和负重",
                                "birth_range": [8, 12],
                                "affects": ["max_carry_weight", "physical_damage"]
                            }
                        }
                    },
                    "status": {
                        "description": "状态值（生理/精神状态）",
                        "attributes": {
                            "hp": {
                                "name": "hp",
                                "type": "status",
                                "display_name": "生命",
                                "description": "Agent的生命值，归零时死亡",
                                "default_value": 100,
                                "min_value": 0,
                                "max_value_formula": "100",
                                "decay_per_tick": 0,
                                "death_condition": {"operator": "equals", "value": 0}
                            },
                            "stamina": {
                                "name": "stamina",
                                "type": "status",
                                "display_name": "体力",
                                "description": "Agent的体力值，用于行动消耗",
                                "default_value": 100,
                                "min_value": 0,
                                "max_value_formula": "100",
                                "decay_per_tick": 5
                            }
                        }
                    },
                    "derived": {
                        "description": "派生属性（基于先天属性计算）",
                        "attributes": {
                            "max_carry_weight": {
                                "name": "max_carry_weight",
                                "type": "derived",
                                "display_name": "最大负重",
                                "description": "可以携带的物品重量上限",
                                "formula": "15 + constitution * 2",
                                "default_value": 30,
                                "min_value": 0,
                                "max_value": 200
                            }
                        }
                    }
                }
            }"#,
        )
        .unwrap();
    }

    #[test]
    fn test_load_unified_attributes() {
        let dir = TempDir::new().unwrap();
        create_test_unified_config(&dir);

        let attrs = load_unified_attributes(dir.path()).unwrap();
        assert_eq!(attrs.version, "0.0.1");
        assert!(attrs.data.primary.attributes.contains_key("strength"));
        assert!(attrs.data.status.attributes.contains_key("hp"));
        assert!(attrs.data.status.attributes.contains_key("stamina"));
        assert!(
            attrs
                .data
                .derived
                .attributes
                .contains_key("max_carry_weight")
        );
    }

    #[test]
    fn test_load_attributes_basic() {
        let dir = TempDir::new().unwrap();
        create_test_unified_config(&dir);

        let attrs = load_attributes(dir.path()).unwrap();
        assert_eq!(attrs.version, "0.0.1");
    }

    #[test]
    fn test_load_unified_attributes_missing_file() {
        let dir = TempDir::new().unwrap();
        // 不创建任何文件

        let result = load_unified_attributes(dir.path());
        assert!(result.is_err());
    }
}
