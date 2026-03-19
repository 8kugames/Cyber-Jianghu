#[cfg(test)]
use crate::game_data::cache::GameDataCache;
#[cfg(test)]
use crate::game_data::registry::init_registry;
#[cfg(test)]
use tempfile::TempDir;

// ============================================================================
// 测试辅助函数
// ============================================================================

/// 全局测试配置注册表
///
/// 包含 GameDataCache 和 TempDir，确保临时目录在测试期间不被删除
#[cfg(test)]
struct TestRegistry {
    cache: std::sync::Arc<GameDataCache>,
    _temp_dir: TempDir, // 使用 _ 前缀，但保留所有权以防止目录被删除
}

#[cfg(test)]
static TEST_REGISTRY: std::sync::OnceLock<TestRegistry> = std::sync::OnceLock::new();

/// 初始化测试用配置注册表
///
/// 使用最小化的测试配置初始化全局注册表。
/// 所有测试共享同一个配置实例，使用 OnceLock 确保只初始化一次。
///
/// # 示例
/// ```rust
/// #[test]
/// fn test_something() {
///     cyber_jianghu_server::game_data::init_test_registry();
///     // 现在可以使用 ActionRegistry, AttributeRegistry 等
/// }
/// ```
#[cfg(test)]
pub fn init_test_registry() {
    use crate::game_data::loader::GameDataLoader;
    use std::fs;

    // 只初始化一次
    TEST_REGISTRY.get_or_init(|| {
        let dir = TempDir::new().unwrap();

        // 创建最小化的 game_rules.json
        fs::write(
            dir.path().join("game_rules.json"),
            r#"{
                "version": "2.0.0",
                "description": "测试用游戏规则",
                "data": {
                    "agent_state": {
                        "tick": { "game_hours_per_tick": 2, "real_seconds_per_tick": 60 },
                        "location": { "spawn_location": "longmen_inn" },
                        "game_time": { "time_ratio": 120, "start_date": "2024-01-01" }
                    },
                    "validation": {
                        "action_validation": { "max_content_length": 500 },
                        "max_agent_name_length": 100,
                        "max_system_prompt_length": 102400,
                        "max_speak_content_length": 500
                    },
                    "world": {
                        "name": "赛博江湖",
                        "description": "测试世界"
                    }
                }
            }"#,
        ).unwrap();

        // 创建最小化的 attributes.json (统一属性配置)
        fs::write(
            dir.path().join("attributes.json"),
            r#"{
                "version": "3.0.0",
                "description": "测试用统一属性配置",
                "data": {
                    "primary": {
                        "description": "先天属性",
                        "attributes": {
                            "strength": { "name": "strength", "display_name": "力量", "description": "力量", "type": "static", "birth_range": [10, 50], "affects": [] },
                            "agility": { "name": "agility", "display_name": "敏捷", "description": "敏捷", "type": "static", "birth_range": [10, 50], "affects": [] },
                            "constitution": { "name": "constitution", "display_name": "根骨", "description": "根骨", "type": "static", "birth_range": [10, 50], "affects": [] },
                            "intelligence": { "name": "intelligence", "display_name": "悟性", "description": "悟性", "type": "static", "birth_range": [10, 50], "affects": [] },
                            "charisma": { "name": "charisma", "display_name": "魅力", "description": "魅力", "type": "static", "birth_range": [10, 50], "affects": [] },
                            "luck": { "name": "luck", "display_name": "福缘", "description": "福缘", "type": "static", "birth_range": [10, 50], "affects": [] }
                        }
                    },
                    "status": {
                        "description": "状态值",
                        "attributes": {
                            "hp": { "name": "hp", "display_name": "生命值", "description": "生命值", "type": "status", "default_value": 100, "min_value": 0, "max_value_formula": "100", "decay_per_tick": 0, "death_condition": { "operator": "equals", "value": 0 } },
                            "stamina": { "name": "stamina", "display_name": "体力", "description": "体力", "type": "status", "default_value": 100, "min_value": 0, "max_value_formula": "100", "decay_per_tick": 0, "recovery_formula": "5 + constitution * 0.1" },
                            "hunger": { "name": "hunger", "display_name": "饥饿", "description": "饥饿", "type": "status", "default_value": 50, "min_value": 0, "max_value_formula": "100", "decay_per_tick": 5, "death_condition": { "operator": "equals", "value": 0 } },
                            "thirst": { "name": "thirst", "display_name": "口渴", "description": "口渴", "type": "status", "default_value": 50, "min_value": 0, "max_value_formula": "100", "decay_per_tick": 5, "death_condition": { "operator": "equals", "value": 0 } },
                            "qi": { "name": "qi", "display_name": "内气", "description": "内气", "type": "status", "default_value": 50, "min_value": 0, "max_value_formula": "100", "decay_per_tick": 0 },
                            "sanity": { "name": "sanity", "display_name": "理智", "description": "理智", "type": "status", "default_value": 100, "min_value": 0, "max_value_formula": "100", "decay_per_tick": 0 },
                            "reputation": { "name": "reputation", "display_name": "声望", "description": "声望", "type": "status", "default_value": 0, "min_value": -1000, "max_value_formula": "1000", "decay_per_tick": 0 }
                        }
                    },
                    "derived": {
                        "description": "派生属性",
                        "attributes": {}
                    }
                }
            }"#,
        ).unwrap();

        // 创建最小化的 items.json
        fs::write(
            dir.path().join("items.json"),
            r#"{
                "version": "0.0.1",
                "description": "测试用物品配置",
                "data": [
                    {
                        "item_id": "mantou",
                        "name": "馒头",
                        "item_type": "consumable",
                        "effects": [
                            {
                                "description": "恢复饥饿值",
                                "attribute": "hunger",
                                "operation": "add",
                                "value": 30
                            }
                        ],
                        "stack_size": 10,
                        "description": "热腾腾的馒头"
                    },
                    {
                        "item_id": "water",
                        "name": "水",
                        "item_type": "consumable",
                        "effects": [
                            {
                                "description": "恢复口渴值",
                                "attribute": "thirst",
                                "operation": "add",
                                "value": 30
                            }
                        ],
                        "stack_size": 10,
                        "description": "清凉的井水"
                    },
                    {
                        "item_id": "silver",
                        "name": "银子",
                        "item_type": "misc",
                        "effects": [],
                        "stack_size": 100,
                        "description": "通用货币"
                    },
                    {
                        "item_id": "knife",
                        "name": "刀",
                        "item_type": "weapon",
                        "effects": [],
                        "stack_size": 1,
                        "description": "一把锋利的刀"
                    }
                ]
            }"#,
        ).unwrap();

        // 创建最小化的 actions.json
        // ActionsData = HashMap<String, ActionConfigEntry>
        // data 字段直接是动作映射，不是 { actions: {} } 结构
        fs::write(
            dir.path().join("actions.json"),
            r#"{
                "version": "0.0.1",
                "description": "测试用动作配置",
                "data": {
                    "attack": {
                        "description": "攻击目标，造成伤害",
                        "base_damage": 10,
                        "weapon_bonus": 5,
                        "weapon_bonus_multiplier": 1.0,
                        "stamina_cost": 5,
                        "validation": {
                            "requires_target": true,
                            "requires_target_alive": true,
                            "required_fields": ["target_agent_id"]
                        },
                        "requirements": [],
                        "effects": []
                    },
                    "steal": {
                        "description": "从目标身上偷取物品",
                        "success_rate": 0.5,
                        "stamina_cost": 5,
                        "validation": {
                            "requires_target": true,
                            "required_fields": ["target_agent_id"]
                        },
                        "requirements": [],
                        "effects": []
                    },
                    "speak": {
                        "description": "公开说话",
                        "max_content_length": 500,
                        "stamina_cost": 0,
                        "validation": {
                            "required_fields": ["content"],
                            "field_validations": [
                                { "field": "content", "validation_type": "not_empty" },
                                { "field": "content", "validation_type": "max_length", "max_length": 500 }
                            ]
                        },
                        "requirements": [],
                        "effects": []
                    },
                    "move": {
                        "description": "移动到指定位置",
                        "stamina_cost": 10,
                        "validation": {
                            "required_fields": ["target_location"],
                            "field_validations": [
                                { "field": "target_location", "validation_type": "not_empty" }
                            ]
                        },
                        "requirements": [],
                        "effects": []
                    },
                    "give": {
                        "description": "将物品给予目标",
                        "stamina_cost": 2,
                        "validation": {
                            "requires_target": true,
                            "requires_target_alive": true,
                            "required_fields": ["target_agent_id", "item_id", "quantity"],
                            "field_validations": [
                                { "field": "quantity", "validation_type": "min_value", "min_value": 1 }
                            ]
                        },
                        "requirements": [],
                        "effects": []
                    },
                    "use": {
                        "description": "使用物品",
                        "stamina_cost": 1,
                        "validation": {
                            "required_fields": ["item_id"]
                        },
                        "requirements": [],
                        "effects": []
                    },
                    "pickup": {
                        "description": "从场景中拾取物品",
                        "stamina_cost": 2,
                        "validation": {
                            "required_fields": ["item_id"]
                        },
                        "requirements": [],
                        "effects": []
                    },
                    "trade": {
                        "description": "与目标进行交易",
                        "stamina_cost": 3,
                        "validation": {
                            "requires_target": true,
                            "required_fields": ["target_agent_id"]
                        },
                        "requirements": [],
                        "effects": []
                    },
                    "idle": {
                        "description": "休息",
                        "stamina_cost": 0,
                        "validation": {},
                        "requirements": [],
                        "effects": []
                    }
                }
            }"#,
        ).unwrap();

        // 创建 initial_inventory.json
        fs::write(
            dir.path().join("initial_inventory.json"),
            r#"{
                "version": "0.0.1",
                "description": "测试用初始物品配置",
                "data": {
                    "items": {
                        "food": [
                            { "item_id": "mantou", "name": "馒头", "quantity": 3, "description": "热腾腾的馒头" },
                            { "item_id": "water", "name": "水", "quantity": 3, "description": "清凉的井水" }
                        ],
                        "currency": [
                            { "item_id": "silver", "name": "银子", "quantity": 10, "description": "通用货币" }
                        ]
                    }
                }
            }"#,
        ).unwrap();

        // 创建 inventory.json
        fs::write(
            dir.path().join("inventory.json"),
            r#"{
                "version": "0.0.1",
                "description": "测试用物品栏配置",
                "data": {
                    "max_slots": 10,
                    "max_stack_size": 10
                }
            }"#,
        ).unwrap();

        // 创建 network.json
        fs::write(
            dir.path().join("network.json"),
            r#"{
                "version": "0.0.1",
                "description": "测试用网络配置",
                "data": {
                    "websocket": {
                        "rate_limit_ms": 500,
                        "cleanup_interval_secs": 300,
                        "cleanup_threshold": 100
                    }
                }
            }"#,
        ).unwrap();

        // 创建 locations.json
        fs::write(
            dir.path().join("locations.json"),
            r#"{
                "version": "0.0.1",
                "description": "测试用位置配置",
                "data": {
                    "nodes": [
                        {
                            "node_id": "inn",
                            "name": "龙门客栈",
                            "type": "map",
                            "parent_id": ""
                        },
                        {
                            "node_id": "lobby",
                            "name": "大堂",
                            "type": "sub_scene",
                            "parent_id": "inn"
                        }
                    ],
                    "edges": []
                }
            }"#,
        ).unwrap();

        // 创建 primary_attributes.json (先天属性)
        fs::write(
            dir.path().join("primary_attributes.json"),
            r#"{
                "version": "0.0.1",
                "description": "先天属性系统（DND风格）",
                "data": {
                    "attributes": {
                        "strength": {
                            "name": "strength",
                            "type": "static",
                            "display_name": "力量",
                            "description": "影响物理攻击和负重",
                            "birth_range": [8, 12],
                            "affects": ["max_carry_weight", "physical_damage"]
                        },
                    "agility": {
                        "name": "agility",
                        "type": "static",
                        "display_name": "敏捷",
                        "description": "影响闪避和移动速度",
                        "birth_range": [8, 12],
                        "affects": ["dodge_rate"]
                    },
                    "constitution": {
                        "name": "constitution",
                        "type": "static",
                        "display_name": "体质",
                        "description": "影响生命值和抗性",
                        "birth_range": [8, 12],
                        "affects": ["max_carry_weight", "hp"]
                    },
                    "intelligence": {
                        "name": "intelligence",
                        "type": "static",
                        "display_name": "智力",
                        "description": "影响技能效果和经验获取",
                        "birth_range": [8, 12],
                        "affects": []
                    },
                    "charisma": {
                        "name": "charisma",
                        "type": "static",
                        "display_name": "魅力",
                        "description": "影响社交和交易",
                        "birth_range": [8, 12],
                        "affects": []
                    },
                    "luck": {
                        "name": "luck",
                        "type": "static",
                        "display_name": "运气",
                        "description": "影响随机事件和掉落",
                        "birth_range": [8, 12],
                        "affects": []
                    }
                }
            }
        }"#,
        ).unwrap();

        // 创建 status_attributes.json (状态值)
        fs::write(
            dir.path().join("status_attributes.json"),
            r#"{
                "version": "0.0.1",
                "description": "测试用状态属性配置",
                "data": {
                    "attributes": {
                    "hp": {
                        "type": "integer",
                        "default_value": 100,
                        "min_value": 0,
                        "max_value": 100,
                        "decay_per_tick": 0,
                        "death_condition": { "operator": "equals", "value": 0 },
                        "display_name": "生命",
                        "description": "Agent的生命值，归零时死亡"
                    },
                    "stamina": {
                        "type": "integer",
                        "default_value": 100,
                        "min_value": 0,
                        "max_value": 100,
                        "decay_per_tick": 5,
                        "display_name": "体力",
                        "description": "Agent的体力值，用于行动消耗"
                    },
                    "hunger": {
                        "type": "integer",
                        "default_value": 50,
                        "min_value": 0,
                        "max_value": 100,
                        "decay_per_tick": -5,
                        "death_condition": { "operator": "equals", "value": 0 },
                        "display_name": "饥饿",
                        "description": "Agent的饥饿值，过低会影响生命值"
                    },
                    "thirst": {
                        "type": "integer",
                        "default_value": 50,
                        "min_value": 0,
                        "max_value": 100,
                        "decay_per_tick": -5,
                        "death_condition": { "operator": "equals", "value": 0 },
                        "display_name": "口渴",
                        "description": "Agent的口渴值，过低会影响生命值"
                    }
                }
            }
        }"#,
        ).unwrap();

        // 创建 derived_attributes.json (派生属性)
        fs::write(
            dir.path().join("derived_attributes.json"),
            r#"{
                "version": "0.0.1",
                "description": "测试用派生属性配置",
                "data": {
                    "attributes": {
                        "max_carry_weight": {
                            "type": "integer",
                            "default_value": 30,
                            "min_value": 0,
                            "max_value": 200,
                            "formula": "15 + constitution * 2",
                            "display_name": "最大负重",
                            "description": "可以携带的物品重量上限"
                        },
                        "physical_damage": {
                            "type": "integer",
                            "default_value": 10,
                            "min_value": 0,
                            "max_value": 50,
                            "formula": "5 + strength * 0.5",
                            "display_name": "物理攻击力",
                            "description": "物理攻击造成的伤害"
                        },
                        "dodge_rate": {
                            "type": "float",
                            "default_value": 0.05,
                            "min_value": 0.0,
                            "max_value": 1.0,
                            "formula": "0.05 + agility * 0.005",
                            "display_name": "闪避率",
                            "description": "躲避攻击的概率"
                        }
                    }
                }
            }"#,
        ).unwrap();

        // 写入 recipes.json
        // RecipesData = HashMap<String, RecipeDefinition>
        // data 字段直接是配方映射，不是 { recipes: {} } 结构
        fs::write(
            dir.path().join("recipes.json"),
            r#"{
                "version": "0.0.1",
                "description": "测试用配方配置",
                "data": {}
            }"#,
        ).unwrap();

        // 写入 time.json
        fs::write(
            dir.path().join("time.json"),
            r#"{
                "version": "2.0.0",
                "description": "测试用时间配置",
                "data": {
                    "ticks_per_hour": 60,
                    "hours_per_day": 24,
                    "days_per_season": 10,
                    "seasons": [
                        {
                            "id": "test_season",
                            "name": "测试季节",
                            "description": "用于测试的季节",
                            "temperature_modifier": 0,
                            "resource_growth_rate": 1.0,
                            "attribute_modifiers": {
                                "hunger": 1.0,
                                "thirst": 1.0,
                                "stamina": 1.0
                            }
                        }
                    ]
                }
            }"#,
        ).unwrap();

        let loader = GameDataLoader::new(dir.path());
        let game_data = loader.load_all().unwrap();
        let cache = std::sync::Arc::new(GameDataCache::new(game_data));

        TestRegistry {
            cache,
            _temp_dir: dir, // 保留 TempDir 所有权，防止目录被删除
        }
    });

    // 初始化全局注册表
    let registry = TEST_REGISTRY.get().unwrap();
    init_registry(registry.cache.clone());
}
