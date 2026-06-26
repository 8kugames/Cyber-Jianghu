// ============================================================================
// OpenClaw Cyber-Jianghu 测试工具模块
// ============================================================================
//
// 本模块提供测试辅助函数和测试配置数据
// ============================================================================

#[cfg(test)]
use std::fs;
#[cfg(test)]
use tempfile::TempDir;

/// 创建测试配置目录
///
/// 生成所有必需的配置文件用于测试
#[cfg(test)]
pub fn create_test_config_dir() -> TempDir {
    let dir = TempDir::new().unwrap();

    // 创建 game_rules.json
    fs::write(
        dir.path().join("game_rules.json"),
        r#"{
            "version": "2.0.0",
            "description": "测试用游戏规则",
            "meta": {},
            "data": {
                "agent_state": {
                    "tick": {
                        "real_seconds_per_tick": 60
                    },
                    "location": {
                        "spawn_location": "龙门客栈"
                    },
                    "game_time": {
                        "start_date": "2024-01-01"
                    }
                },
                "validation": {
                    "action_validation": {
                        "max_content_length": 1000
                    },
                    "max_agent_name_length": 100,
                    "max_system_prompt_length": 102400,
                    "max_speak_content_length": 500
                },
                "ops": {
                    "death_threshold": 10,
                    "offline_cleanup_days": 30
                },
                "world": {
                    "name": "虚境：江湖",
                    "description": "一个充满武侠与科技的世界"
                }
            }
        }"#,
    )
    .unwrap();

    // 创建 items.json
    fs::write(
        dir.path().join("items.json"),
        r#"{
            "version": "0.0.1",
            "items": [
                {
                    "item_id": "馒头",
                    "name": "馒头",
                    "item_type": "consumable",
                    "effects": [
                        {
                            "description": "恢复饱食度",
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

    // 创建 actions.json
    fs::write(
        dir.path().join("actions.json"),
        r#"{
            "version": "0.0.1",
            "actions": {
                "攻击": { "base_damage": 10 },
                "取": {}  // 取无全局 success_rate，授权由 validator 处理
            }
        }"#,
    )
    .unwrap();

    // 创建 initial_inventory.json
    fs::write(
        dir.path().join("initial_inventory.json"),
        r#"{
            "version": "0.0.1",
            "description": "测试用初始物品配置",
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

    // 创建 inventory.json
    fs::write(
        dir.path().join("inventory.json"),
        r#"{
            "version": "0.0.1",
            "limits": {
                "max_slots": 10,
                "max_stack_size": 10
            }
        }"#,
    )
    .unwrap();

    // 创建 network.json
    fs::write(
        dir.path().join("network.json"),
        r#"{
            "version": "0.0.1",
            "websocket": {
                "rate_limit_ms": 500,
                "cleanup_interval_secs": 300,
                "cleanup_threshold": 100
            }
        }"#,
    )
    .unwrap();

    // 创建 primary_attributes.json (先天属性)
    fs::write(
        dir.path().join("primary_attributes.json"),
        r#"{
            "version": "0.0.1",
            "description": "先天属性系统（DND风格）",
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
        }"#,
    )
    .unwrap();

    // 创建 status_attributes.json (状态值)
    fs::write(
        dir.path().join("status_attributes.json"),
        r#"{
            "version": "0.0.1",
            "attributes": {
                "hp": {
                    "type": "integer",
                    "default_value": 100,
                    "min_value": 0,
                    "max_value": 100,
                    "decay_per_tick": 0,
                    "death_condition": {"operator": "equals", "value": 0},
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
                "satiation": {
                    "type": "integer",
                    "default_value": 50,
                    "min_value": 0,
                    "max_value": 100,
                    "decay_per_tick": -5,
                    "death_condition": {"operator": "equals", "value": 0},
                    "display_name": "饱食度",
                    "description": "Agent的饱食程度，过低会影响生命值"
                },
                "hydration": {
                    "type": "integer",
                    "default_value": 50,
                    "min_value": 0,
                    "max_value": 100,
                    "decay_per_tick": -5,
                    "death_condition": {"operator": "equals", "value": 0},
                    "display_name": "饱饮度",
                    "description": "Agent的饮水程度，过低会影响生命值"
                }
            }
        }"#,
    )
    .unwrap();

    // 创建 derived_attributes.json (派生属性)
    fs::write(
        dir.path().join("derived_attributes.json"),
        r#"{
            "version": "0.0.1",
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
        }"#,
    )
    .unwrap();

    // 创建 locations.json
    fs::write(
        dir.path().join("locations.json"),
        r#"{
            "version": "0.0.1",
            "nodes": [
                {
                    "node_id": "inn",
                    "name": "龙门客栈",
                    "type": "map"
                },
                {
                    "node_id": "lobby",
                    "name": "大堂",
                    "type": "sub_scene",
                    "parent_id": "inn"
                }
            ],
            "edges": []
        }"#,
    )
    .unwrap();

    // 创建 attributes.json (统一属性配置)
    fs::write(
        dir.path().join("attributes.json"),
        r#"{
            "version": "3.0.0",
            "description": "统一属性配置文件（数据驱动架构）",
            "categories": {
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
                },
                "status": {
                    "description": "状态值",
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
                        },
                        "satiation": {
                            "name": "satiation",
                            "type": "status",
                            "display_name": "饱食度",
                            "description": "Agent的饱食程度，过低会影响生命值",
                            "default_value": 50,
                            "min_value": 0,
                            "max_value_formula": "100",
                            "decay_per_tick": -5,
                            "death_condition": {"operator": "equals", "value": 0}
                        },
                        "hydration": {
                            "name": "hydration",
                            "type": "status",
                            "display_name": "饱饮度",
                            "description": "Agent的饮水程度，过低会影响生命值",
                            "default_value": 50,
                            "min_value": 0,
                            "max_value_formula": "100",
                            "decay_per_tick": -5,
                            "death_condition": {"operator": "equals", "value": 0}
                        }
                    }
                },
                "derived": {
                    "description": "派生属性",
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
                        },
                        "physical_damage": {
                            "name": "physical_damage",
                            "type": "derived",
                            "display_name": "物理攻击力",
                            "description": "物理攻击造成的伤害",
                            "formula": "5 + strength * 0.5",
                            "default_value": 10,
                            "min_value": 0,
                            "max_value": 50
                        },
                        "dodge_rate": {
                            "name": "dodge_rate",
                            "type": "derived",
                            "display_name": "闪避率",
                            "description": "躲避攻击的概率",
                            "formula": "0.05 + agility * 0.005",
                            "default_value": 0.05,
                            "min_value": 0.0,
                            "max_value": 1.0
                        }
                    }
                }
            }
        }"#,
    )
    .unwrap();

    // 创建 reward.yaml（生存 Reward 配置，强制配置）
    fs::write(
        dir.path().join("reward.yaml"),
        r#"
version: "0.0.1"
description: "测试用 reward 配置"
daily:
  survival_score: 1.0
  physiological:
    satiation_weight: 0.25
    hydration_weight: 0.25
  tianhun:
    approved_score: 0.5
    rejected_score: -0.5
lifetime:
  death_penalty: -50.0
output:
  enabled: true
  base_dir: "rewards"
  flush_on_death: true
"#,
    )
    .unwrap();

    dir
}
