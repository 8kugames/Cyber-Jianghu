//! 共享类型定义
//!
//! 这些类型在服务端和客户端之间共享，确保协议一致性。
//!
//! ## 核心类型
//!
//! - [`Intent`] - Agent 意图 (每 Tick 上报一次)
//! - [`ActionType`] - 动作类型 (idle, speak, move, attack, give, craft)
//! - [`WorldState`] - 世界状态快照 (包含所有 Agent 状态)
//! - [`AgentSelfState`] - Agent 状态 (HP、位置、物品清单等)
//! - [`LocationNode`] - 位置节点 (场景、地点)
//! - [`GameRules`] - 游戏规则 (初始状态、衰减率、时间转换)
//!
//! ## 子模块
//!
//! - `attributes`: 属性系统相关
//! - `actions`: 动作和意图相关
//! - `entities`: 实体相关（Agent, Item, Scene）
//! - `world`: 世界状态、时间、事件相关
//! - `locations`: 位置图相关
//! - `rules`: 游戏规则相关
//! - `narrative`: 叙事化配置相关

// 子模块声明
pub mod actions;
pub mod attributes;
pub mod entities;
pub mod locations;
pub mod narrative;
pub mod prompt_template;
pub mod review;
pub mod rules;
pub mod world;

// 重导出所有公共类型
pub use actions::*;
pub use attributes::*;
pub use entities::*;
pub use locations::*;
pub use narrative::*;
pub use prompt_template::*;
pub use review::*;
pub use rules::*;
pub use world::*;

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use uuid::Uuid;

    #[test]
    fn test_action_type_serde() {
        let idle = ActionType::new("休息");
        assert_eq!(serde_json::to_string(&idle).unwrap(), "\"休息\"");
        let speak = ActionType::new("说话");
        assert_eq!(serde_json::to_string(&speak).unwrap(), "\"说话\"");
        let custom = ActionType::new("打坐");
        assert_eq!(serde_json::to_string(&custom).unwrap(), "\"打坐\"");
    }

    #[test]
    fn test_action_type_from_str() {
        let action: ActionType = "休息".into();
        assert_eq!(action.as_str(), "休息");
        let action: ActionType = "custom_action".into();
        assert_eq!(action.as_str(), "custom_action");
    }

    #[test]
    fn test_intent_creation() {
        let agent_id = Uuid::new_v4();
        let intent = Intent::new(agent_id, 1, "休息", None);
        assert_eq!(intent.action_type.as_str(), "休息");
        assert_eq!(intent.tick_id, 1);

        let intent = Intent::new(
            agent_id,
            2,
            "说话",
            Some(serde_json::json!({"content": "大家好"})),
        );
        assert_eq!(intent.action_type.as_str(), "说话");
        assert!(intent.action_data.is_some());
    }

    #[test]
    fn test_intent_custom_action() {
        let agent_id = Uuid::new_v4();
        let intent = Intent::new(
            agent_id,
            1,
            "打坐",
            Some(serde_json::json!({ "duration": 60 })),
        );
        assert_eq!(intent.action_type.as_str(), "打坐");
        assert!(intent.action_data.is_some());
    }

    #[test]
    fn test_intent_with_thought() {
        let agent_id = Uuid::new_v4();
        let intent =
            Intent::new(agent_id, 1, "休息", None).with_thought("我需要休息一下".to_string());
        assert_eq!(intent.thought_log, Some("我需要休息一下".to_string()));
    }

    #[test]
    fn test_world_state_serde() {
        let world_state = WorldState {
            event_type: "world_state".to_string(),
            tick_id: 1,
            agent_id: None,
            world_time: WorldTime {
                year: 2024,
                month: 3,
                day: 15,
                hour: 12,
                minute: 0,
                second: 0,
                weather: "晴".to_string(),
            },
            location: Location {
                node_id: "test".to_string(),
                name: "Test".to_string(),
                node_type: "客栈".to_string(),
                adjacent_nodes: vec![],
                gatherable_items: vec![],
            },
            self_state: AgentSelfState {
                attributes: {
                    let mut attrs = std::collections::HashMap::new();
                    attrs.insert("hp".to_string(), 100);
                    attrs.insert("stamina".to_string(), 100);
                    attrs.insert("hunger".to_string(), 50);
                    attrs.insert("thirst".to_string(), 50);
                    attrs
                },
                derived_attributes: std::collections::HashMap::new(),
                attribute_descriptions: std::collections::HashMap::new(),
                status_effects: vec![],
                inventory: vec![],
                skills: vec![],
                age_years: None,
                max_age: None,
                recipe_details: vec![],
            },
            entities: vec![],
            nearby_items: vec![],
            events_log: vec![],
            private_dialogue_log: vec![],
            last_execution_summary: None,
            lessons_learned: vec![],
        };

        let json = serde_json::to_string(&world_state).unwrap();
        let parsed: WorldState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.tick_id, 1);
    }

    #[test]
    fn test_location_node_serialization() {
        let node = LocationNode {
            node_id: "龙门大堂".to_string(),
            name: "大堂".to_string(),
            node_type: LocationNodeType::SubScene,
            parent_id: Some("龙门客栈".to_string()),
            environmental_damage: None,
            gatherable_items: vec![],
            implicit_travel_cost: None,
            aliases: vec!["longmen_lobby".to_string()],
        };

        let json = serde_json::to_string(&node).unwrap();
        assert!(json.contains("sub_scene"));

        let parsed: LocationNode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.node_id, "龙门大堂");
    }

    #[test]
    fn test_location_graph() {
        let mut graph = LocationGraph::new();

        graph.add_node(LocationNode {
            node_id: "龙门客栈".to_string(),
            name: "龙门客栈".to_string(),
            node_type: LocationNodeType::Map,
            parent_id: None,
            environmental_damage: None,
            gatherable_items: vec![],
            implicit_travel_cost: None,
            aliases: vec!["longmen_inn".to_string()],
        });

        graph.add_node(LocationNode {
            node_id: "龙门大堂".to_string(),
            name: "大堂".to_string(),
            node_type: LocationNodeType::SubScene,
            parent_id: Some("龙门客栈".to_string()),
            environmental_damage: None,
            gatherable_items: vec![],
            implicit_travel_cost: None,
            aliases: vec!["longmen_lobby".to_string()],
        });

        graph.add_node(LocationNode {
            node_id: "龙门后院".to_string(),
            name: "后院".to_string(),
            node_type: LocationNodeType::SubScene,
            parent_id: Some("龙门客栈".to_string()),
            environmental_damage: None,
            gatherable_items: vec![],
            implicit_travel_cost: None,
            aliases: vec!["longmen_backyard".to_string()],
        });

        graph.add_edge(LocationEdge {
            from_node_id: "龙门大堂".to_string(),
            to_node_id: "龙门后院".to_string(),
            travel_cost: 1,
        });

        // 显式边
        assert!(graph.is_connected("龙门大堂", "龙门后院"));
        // 隐式 parent-child 连接
        assert!(graph.is_connected("龙门大堂", "龙门客栈"));
        assert!(graph.is_connected("龙门客栈", "龙门大堂"));
        assert!(graph.is_connected("龙门客栈", "龙门后院"));
        // 无连接
        assert!(!graph.is_connected("龙门后院", "龙门大堂")); // 无显式反向边

        let neighbors = graph.get_neighbors("龙门大堂");
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0].to_node_id, "龙门后院");

        // 隐式邻居
        let implicit = graph.get_implicit_neighbors("龙门大堂", 1);
        assert_eq!(implicit.len(), 1); // parent: 龙门客栈
        assert_eq!(implicit[0].node_id, "龙门客栈");

        // 全部邻居（显式+隐式）
        let all = graph.get_all_neighbors("龙门大堂", 1);
        assert_eq!(all.len(), 2); // 龙门后院 (explicit) + 龙门客栈 (implicit)
    }

    #[test]
    fn test_world_building_rules_construction() {
        let rules = WorldBuildingRules {
            version: "0.0.1".to_string(),
            era: EraSettings {
                name: "武侠架空世界".to_string(),
                tech_level: "冷兵器时代".to_string(),
                social_structure: "封建帝制".to_string(),
            },
            allowed_concepts: vec!["内力".to_string(), "轻功".to_string()],
            forbidden_concepts: vec!["魔法".to_string()],
            narrative_rules: "测试".to_string(),
            last_updated: "2026-01-01T00:00:00Z".to_string(),
        };
        assert_eq!(rules.version, "0.0.1");
        assert!(rules.allowed_concepts.contains(&"内力".to_string()));
        assert!(rules.forbidden_concepts.contains(&"魔法".to_string()));
    }

    #[test]
    fn test_world_building_rules_serde() {
        let rules = WorldBuildingRules {
            version: "0.0.1".to_string(),
            era: EraSettings {
                name: "测试时代".to_string(),
                tech_level: "测试技术".to_string(),
                social_structure: "测试社会".to_string(),
            },
            allowed_concepts: vec!["概念1".into()],
            forbidden_concepts: vec!["禁止1".into()],
            narrative_rules: "测试规则".to_string(),
            last_updated: "2024-01-01T00:00:00Z".to_string(),
        };

        let json = serde_json::to_string(&rules).unwrap();
        let parsed: WorldBuildingRules = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.version, "0.0.1");
        assert_eq!(parsed.era.name, "测试时代");
        assert_eq!(parsed.allowed_concepts.len(), 1);
    }
}
