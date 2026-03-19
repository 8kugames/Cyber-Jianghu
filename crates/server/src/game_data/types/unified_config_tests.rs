// ============================================================================
// 统一配置测试
// ============================================================================

#[cfg(test)]
mod unified_config_tests {
    use super::*;

    /// 创建测试用的统一配置
    fn create_test_unified_config() -> UnifiedAttributesConfig {
        use std::collections::HashMap;

        UnifiedAttributesConfig {
            version: "3.0.0".to_string(),
            description: "测试统一属性配置".to_string(),
            categories: AttributeCategories {
                primary: PrimaryAttributesCategory {
                    description: "先天属性".to_string(),
                    attributes: {
                        let mut attrs = HashMap::new();
                        // 注意：这里不能直接创建 PrimaryAttributeDefinition 因为它使用旧类型
                        // 在实际测试中应该从 JSON 加载
                        attrs
                    },
                },
                status: StatusAttributesCategory {
                    description: "状态值".to_string(),
                    attributes: {
                        let mut attrs = HashMap::new();
                        attrs
                    },
                },
                derived: DerivedAttributesCategory {
                    description: "派生属性".to_string(),
                    attributes: {
                        let mut attrs = HashMap::new();
                        attrs
                    },
                },
            },
        }
    }

    #[test]
    fn test_unified_config_from_json() {
        let json = r#"{
            "version": "3.0.0",
            "description": "统一属性配置文件（数据驱动架构）",
            "categories": {
                "primary": {
                    "description": "先天属性（出生时决定，部分可成长）",
                    "attributes": {
                        "strength": {
                            "name": "strength",
                            "display_name": "力量",
                            "description": "负重能力、外功物理伤害",
                            "type": "growable",
                            "birth_range": [10, 50],
                            "initial_value": 10,
                            "growth_rate": 1.0,
                            "affects": ["max_carry_weight", "physical_damage"]
                        }
                    }
                },
                "status": {
                    "description": "状态值（生理/精神状态，会随时间衰减或恢复）",
                    "attributes": {
                        "hp": {
                            "name": "hp",
                            "display_name": "生命值",
                            "description": "Agent的生命值，受根骨影响，降为0时死亡",
                            "type": "status",
                            "formula": "100 + constitution * 2",
                            "default_value": 100,
                            "min_value": 0,
                            "max_value_formula": "100 + constitution * 2",
                            "decay_per_tick": 0,
                            "primary_attribute_deps": ["constitution"]
                        }
                    }
                },
                "derived": {
                    "description": "派生属性（基于先天属性实时计算，不存储）",
                    "attributes": {
                        "max_carry_weight": {
                            "name": "max_carry_weight",
                            "display_name": "负重上限",
                            "description": "Agent可携带的物品重量上限，受力量影响",
                            "type": "derived",
                            "formula": "50 + strength * 2",
                            "default_value": 50,
                            "min_value": 0,
                            "primary_attribute_deps": ["strength"]
                        }
                    }
                }
            }
        }"#;

        let config: UnifiedAttributesConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.version, "3.0.0");

        // 验证主属性
        assert!(config.data.primary.attributes.contains_key("strength"));
        let strength = &config.data.primary.attributes["strength"];
        assert_eq!(strength.display_name, "力量");

        // 验证状态值
        assert!(config.data.status.attributes.contains_key("hp"));
        let hp = &config.data.status.attributes["hp"];
        assert_eq!(hp.display_name, "生命值");
        assert_eq!(hp.default_value, Some(100.0));

        // 验证派生属性
        assert!(config.data.derived.attributes.contains_key("max_carry_weight"));
    }

    #[test]
    fn test_attribute_component_from_unified_config() {
        use crate::game_data::formula_engine::FormulaEngine;

        let json = r#"{
            "version": "0.0.1",
            "description": "测试统一属性配置",
            "meta": {},
            "data": {
                "primary": {
                    "description": "先天属性",
                    "attributes": {
                        "strength": {
                            "name": "strength",
                            "display_name": "力量",
                            "description": "负重能力",
                            "type": "growable",
                            "birth_range": [10, 50],
                            "initial_value": 30,
                            "growth_rate": 1.0,
                            "affects": []
                        },
                        "constitution": {
                            "name": "constitution",
                            "display_name": "根骨",
                            "description": "生存韧性",
                            "type": "growable",
                            "birth_range": [10, 50],
                            "initial_value": 20,
                            "growth_rate": 0.8,
                            "affects": []
                        }
                    }
                },
                "status": {
                    "description": "状态值",
                    "attributes": {
                        "hp": {
                            "name": "hp",
                            "display_name": "生命值",
                            "description": "Agent的生命值",
                            "type": "status",
                            "formula": "100 + constitution * 2",
                            "default_value": 100,
                            "min_value": 0,
                            "max_value_formula": "100 + constitution * 2",
                            "decay_per_tick": 0,
                            "primary_attribute_deps": ["constitution"]
                        }
                    }
                },
                "derived": {
                    "description": "派生属性",
                    "attributes": {}
                }
            }
        }"#;

        let config: UnifiedAttributesConfig = serde_json::from_str(json).unwrap();

        // 测试从统一配置创建主属性组件
        let primary = AttributeComponent::from_unified_config(&config);
        assert_eq!(primary.get_value("strength"), Some(30));
        assert_eq!(primary.get_value("constitution"), Some(20));

        // 测试从统一配置创建状态值组件
        let status = StatusComponent::from_unified_config(&config);
        assert_eq!(status.get("hp"), Some(100));
    }

    #[test]
    fn test_unified_config_formula_evaluation() {
        use crate::game_data::formula_engine::FormulaEngine;

        let json = r#"{
            "version": "0.0.1",
            "description": "测试公式计算",
            "meta": {},
            "data": {
                "primary": {
                    "description": "先天属性",
                    "attributes": {
                        "strength": {
                            "name": "strength",
                            "display_name": "力量",
                            "type": "growable",
                            "birth_range": [10, 50],
                            "initial_value": 30,
                            "growth_rate": 1.0,
                            "affects": []
                        },
                        "constitution": {
                            "name": "constitution",
                            "display_name": "根骨",
                            "type": "growable",
                            "birth_range": [10, 50],
                            "initial_value": 20,
                            "growth_rate": 0.8,
                            "affects": []
                        }
                    }
                },
                "status": {
                    "description": "状态值",
                    "attributes": {
                        "hp": {
                            "name": "hp",
                            "display_name": "生命值",
                            "type": "status",
                            "formula": "100 + constitution * 2",
                            "default_value": 100,
                            "min_value": 0,
                            "max_value_formula": "100 + constitution * 2",
                            "decay_per_tick": 0,
                            "primary_attribute_deps": ["constitution"]
                        }
                    }
                },
                "derived": {
                    "description": "派生属性",
                    "attributes": {
                        "physical_damage": {
                            "name": "physical_damage",
                            "display_name": "物理伤害",
                            "type": "derived",
                            "formula": "10 + strength * 0.5",
                            "default_value": 10,
                            "min_value": 0,
                            "primary_attribute_deps": ["strength"]
                        }
                    }
                }
            }
        }"#;

        let config: UnifiedAttributesConfig = serde_json::from_str(json).unwrap();
        let primary = AttributeComponent::from_unified_config(&config);
        let engine = FormulaEngine::new();

        // 测试公式计算：100 + constitution * 2 = 100 + 20 * 2 = 140
        let hp_formula = &config.data.status.attributes["hp"].formula.as_ref().unwrap();
        let hp_value = engine.evaluate(hp_formula, &primary).unwrap();
        assert_eq!(hp_value, 140.0);

        // 测试派生属性公式：10 + strength * 0.5 = 10 + 30 * 0.5 = 25
        let damage_formula = &config.data.derived.attributes["physical_damage"].formula.as_ref().unwrap();
        let damage_value = engine.evaluate(damage_formula, &primary).unwrap();
        assert_eq!(damage_value, 25.0);
    }
}
