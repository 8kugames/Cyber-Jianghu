// ============================================================================
// 状态查询工具定义与执行
// ============================================================================
//
// 3 个 EarthSoul 工具，将 prompt 内嵌数据替换为按需 tool calling：
// - get_action_detail: 查询指定动作的详细信息
// - query_world: 查询 WorldState 指定分区
// - list_skills: 列出已掌握技能索引
//
// 设计原则：progressive disclosure — prompt 只注入最小摘要，LLM 自主判断何时加载详情。

use crate::component::llm::tool_types::ToolDefinition;
use crate::component::state_store::WorldStateStore;
use cyber_jianghu_protocol::AvailableAction;

/// get_action_detail tool 定义
pub fn get_action_detail_definition() -> ToolDefinition {
    ToolDefinition::new(
        "get_action_detail",
        "查询指定动作的详细信息，包括描述、分类、所需字段和有效目标。当需要了解某个动作的具体用法时调用。",
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "action_type": {
                    "type": "string",
                    "description": "动作类型名称（如 attack, gather, craft 等），支持别名匹配"
                }
            },
            "required": ["action_type"]
        })),
    )
}

/// query_world tool 定义
pub fn query_world_definition() -> ToolDefinition {
    ToolDefinition::new(
        "query_world",
        "查询当前世界状态的指定部分。可用于获取背包物品、附近实体、环境信息、自身属性或事件日志。",
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "section": {
                    "type": "string",
                    "enum": ["inventory", "entities", "environment", "state", "events"],
                    "description": "查询的世界状态部分：inventory=背包, entities=附近实体, environment=环境/位置, state=自身属性, events=事件日志"
                },
                "filter": {
                    "type": "string",
                    "description": "可选过滤条件（如物品类型 food、实体名称片段等）"
                }
            },
            "required": ["section"]
        })),
    )
}

/// list_skills tool 定义
pub fn list_skills_definition() -> ToolDefinition {
    ToolDefinition::simple(
        "list_skills",
        "列出已掌握的所有技能（仅名称和简要说明）。查看具体技能行为指引请使用 skill_view。",
    )
}

/// 执行 get_action_detail
///
/// 查找顺序：精确匹配 action 字段 → 精确匹配 name 字段 → 别名匹配
pub fn execute_get_action_detail(
    action_type: &str,
    available_actions: &[AvailableAction],
) -> serde_json::Value {
    let action = available_actions
        .iter()
        .find(|a| a.action == action_type || a.name == action_type)
        .or_else(|| {
            available_actions
                .iter()
                .find(|a| a.aliases.iter().any(|alias| alias == action_type))
        });

    match action {
        Some(a) => serde_json::json!({
            "success": true,
            "action": a.action,
            "name": a.name,
            "description": a.description,
            "category": a.category,
            "required_fields": a.required_fields,
            "valid_targets": a.valid_targets,
            "requirements": a.requirements,
            "effects": a.effects,
        }),
        None => serde_json::json!({
            "success": false,
            "message": format!("未找到动作: {}。请使用可用动作列表中的名称。", action_type)
        }),
    }
}

/// 执行 query_world
///
/// 按分区返回 WorldState 子集，支持可选 filter 过滤
pub async fn execute_query_world(
    section: &str,
    filter: Option<&str>,
    store: &WorldStateStore,
) -> serde_json::Value {
    let ws = match store.current().await {
        Some(ws) => ws,
        None => return serde_json::json!({ "success": false, "message": "WorldState 尚未初始化" }),
    };

    match section {
        "inventory" => {
            let items: Vec<_> = ws
                .self_state
                .inventory
                .iter()
                .filter(|item| {
                    filter.is_none_or(|f| item.name.contains(f) || item.item_id.contains(f))
                })
                .map(|item| {
                    serde_json::json!({
                        "item_id": item.item_id,
                        "name": item.name,
                        "quantity": item.quantity,
                        "item_type": item.item_type,
                    })
                })
                .collect();
            serde_json::json!({
                "success": true,
                "section": "inventory",
                "items": items,
                "total": ws.self_state.inventory.len(),
            })
        }
        "entities" => {
            let entities: Vec<_> = ws
                .entities
                .iter()
                .filter(|e| filter.is_none_or(|f| e.name.contains(f)))
                .map(|e| {
                    serde_json::json!({
                        "id": e.id,
                        "name": e.name,
                        "state": e.state,
                    })
                })
                .collect();
            serde_json::json!({
                "success": true,
                "section": "entities",
                "entities": entities,
            })
        }
        "environment" => serde_json::json!({
            "success": true,
            "section": "environment",
            "location": {
                "node_id": ws.location.node_id,
                "name": ws.location.name,
            },
            "nearby_items_count": ws.nearby_items.len(),
            "tick_id": ws.tick_id,
            "world_time": ws.world_time.to_chinese(),
        }),
        "state" => {
            let attrs: serde_json::Map<String, serde_json::Value> = ws
                .self_state
                .attributes
                .iter()
                .filter(|(k, _)| filter.is_none_or(|f| k.contains(f)))
                .map(|(k, &v)| (k.clone(), serde_json::json!(v)))
                .collect();
            serde_json::json!({
                "success": true,
                "section": "state",
                "attributes": attrs,
                "status_effects": ws.self_state.status_effects,
                "skills": ws.self_state.skills.iter().map(|s| &s.name).collect::<Vec<_>>(),
            })
        }
        "events" => {
            let events: Vec<_> = ws
                .events_log
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "tick_id": e.tick_id,
                        "description": e.description,
                    })
                })
                .collect();
            serde_json::json!({
                "success": true,
                "section": "events",
                "events": events,
            })
        }
        _ => serde_json::json!({
            "success": false,
            "message": format!(
                "未知 section: {}。可选: inventory, entities, environment, state, events",
                section
            )
        }),
    }
}

/// 执行 list_skills
///
/// 返回已掌握技能的索引列表（仅 skill_id 和 name）
pub fn execute_list_skills(
    skill_cache: &std::collections::HashMap<String, String>,
) -> serde_json::Value {
    let skills: Vec<_> = skill_cache
        .keys()
        .map(|id| serde_json::json!({ "skill_id": id }))
        .collect();
    serde_json::json!({
        "success": true,
        "skills": skills,
        "total": skills.len(),
        "hint": "使用 skill_view(skill_id) 查看具体技能的行为指引"
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_jianghu_protocol::{AgentSelfState, Location, WorldState, WorldTime};
    use std::collections::HashMap;
    use uuid::Uuid;

    // ---- get_action_detail ----

    fn make_action(action: &str, name: &str, aliases: Vec<&str>) -> AvailableAction {
        AvailableAction {
            action: action.to_string(),
            name: name.to_string(),
            description: format!("{}描述", name),
            category: "test".to_string(),
            valid_targets: None,
            required_fields: vec![],
            ooc_risk: "low".to_string(),
            aliases: aliases.into_iter().map(|s| s.to_string()).collect(),
            field_aliases: HashMap::new(),
            requirements: vec![],
            effects: vec![],
        }
    }

    #[test]
    fn test_get_action_detail_exact_match() {
        let actions = vec![
            make_action("attack", "攻击", vec!["打"]),
            make_action("gather", "采集", vec!["采"]),
        ];
        let result = execute_get_action_detail("attack", &actions);
        assert!(result["success"].as_bool().unwrap());
        assert_eq!(result["action"], "attack");
        assert_eq!(result["name"], "攻击");
    }

    #[test]
    fn test_get_action_detail_name_match() {
        let actions = vec![make_action("attack", "攻击", vec!["打"])];
        let result = execute_get_action_detail("攻击", &actions);
        assert!(result["success"].as_bool().unwrap());
        assert_eq!(result["action"], "attack");
    }

    #[test]
    fn test_get_action_detail_alias_match() {
        let actions = vec![make_action("attack", "攻击", vec!["打", "攻击目标"])];
        let result = execute_get_action_detail("打", &actions);
        assert!(result["success"].as_bool().unwrap());
        assert_eq!(result["action"], "attack");
    }

    #[test]
    fn test_get_action_detail_not_found() {
        let actions = vec![make_action("attack", "攻击", vec![])];
        let result = execute_get_action_detail("nonexistent", &actions);
        assert!(!result["success"].as_bool().unwrap());
        assert!(result["message"].as_str().unwrap().contains("nonexistent"));
    }

    // ---- list_skills ----

    #[test]
    fn test_list_skills_empty() {
        let cache = HashMap::new();
        let result = execute_list_skills(&cache);
        assert!(result["success"].as_bool().unwrap());
        assert_eq!(result["total"], 0);
    }

    #[test]
    fn test_list_skills_with_entries() {
        let mut cache = HashMap::new();
        cache.insert(
            "social/trust-reading".to_string(),
            "识人之明指引".to_string(),
        );
        cache.insert(
            "cognitive/risk-assessment".to_string(),
            "审时度势指引".to_string(),
        );
        let result = execute_list_skills(&cache);
        assert!(result["success"].as_bool().unwrap());
        assert_eq!(result["total"], 2);
    }

    // ---- query_world ----

    fn make_test_world_state() -> WorldState {
        WorldState {
            event_type: "world_state".to_string(),
            tick_id: 42,
            agent_id: Some(Uuid::new_v4()),
            world_time: WorldTime {
                year: 1,
                month: 1,
                day: 1,
                hour: 6,
                minute: 0,
                second: 0,
                weather: String::new(),
            },
            location: Location {
                node_id: "village_square".to_string(),
                name: "村口广场".to_string(),
                node_type: "town".to_string(),
                adjacent_nodes: vec![],
                gatherable_items: vec![],
            },
            self_state: AgentSelfState {
                attributes: {
                    let mut m = HashMap::new();
                    m.insert("hp".to_string(), 100);
                    m.insert("stamina".to_string(), 80);
                    m
                },
                derived_attributes: HashMap::new(),
                attribute_descriptions: HashMap::new(),
                status_effects: vec!["健康".to_string()],
                inventory: vec![cyber_jianghu_protocol::InventoryItem {
                    item_id: "mantou".to_string(),
                    name: "馒头".to_string(),
                    quantity: 3,
                    is_equipped: false,
                    item_type: "consumable".to_string(),
                    aliases: vec![],
                }],
                skills: vec![],
                recipe_details: vec![],
                age_years: None,
                max_age: None,
            },
            entities: vec![cyber_jianghu_protocol::Entity {
                id: Uuid::new_v4(),
                name: "路人甲".to_string(),
                distance: 0,
                state: "alive".to_string(),
                hostile: false,
                recent_actions: vec![],
            }],
            nearby_items: vec![],
            events_log: vec![cyber_jianghu_protocol::WorldEvent {
                event_type: cyber_jianghu_protocol::WorldEventType::ActionResult,
                tick_id: 41,
                description: "你采集了一些野果".to_string(),
                metadata: serde_json::json!({}),
            }],
            private_dialogue_log: vec![],
            last_execution_summary: None,
            lessons_learned: vec![],
        }
    }

    #[tokio::test]
    async fn test_query_world_inventory() {
        let store = WorldStateStore::new();
        store.update(make_test_world_state()).await;
        let result = execute_query_world("inventory", None, &store).await;
        assert!(result["success"].as_bool().unwrap());
        assert_eq!(result["total"], 1);
        let items = result["items"].as_array().unwrap();
        assert_eq!(items[0]["name"], "馒头");
    }

    #[tokio::test]
    async fn test_query_world_inventory_with_filter() {
        let store = WorldStateStore::new();
        store.update(make_test_world_state()).await;
        let result = execute_query_world("inventory", Some("馒头"), &store).await;
        assert!(result["success"].as_bool().unwrap());
        let items = result["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn test_query_world_entities() {
        let store = WorldStateStore::new();
        store.update(make_test_world_state()).await;
        let result = execute_query_world("entities", None, &store).await;
        assert!(result["success"].as_bool().unwrap());
        let entities = result["entities"].as_array().unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0]["name"], "路人甲");
    }

    #[tokio::test]
    async fn test_query_world_environment() {
        let store = WorldStateStore::new();
        store.update(make_test_world_state()).await;
        let result = execute_query_world("environment", None, &store).await;
        assert!(result["success"].as_bool().unwrap());
        assert_eq!(result["location"]["node_id"], "village_square");
        assert_eq!(result["tick_id"], 42);
    }

    #[tokio::test]
    async fn test_query_world_state() {
        let store = WorldStateStore::new();
        store.update(make_test_world_state()).await;
        let result = execute_query_world("state", None, &store).await;
        assert!(result["success"].as_bool().unwrap());
        let attrs = result["attributes"].as_object().unwrap();
        assert_eq!(attrs["hp"], 100);
        assert_eq!(attrs["stamina"], 80);
    }

    #[tokio::test]
    async fn test_query_world_events() {
        let store = WorldStateStore::new();
        store.update(make_test_world_state()).await;
        let result = execute_query_world("events", None, &store).await;
        assert!(result["success"].as_bool().unwrap());
        let events = result["events"].as_array().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["description"], "你采集了一些野果");
    }

    #[tokio::test]
    async fn test_query_world_unknown_section() {
        let store = WorldStateStore::new();
        store.update(make_test_world_state()).await;
        let result = execute_query_world("nonexistent", None, &store).await;
        assert!(!result["success"].as_bool().unwrap());
        assert!(result["message"].as_str().unwrap().contains("nonexistent"));
    }

    // ---- tool definitions ----

    #[test]
    fn test_tool_definitions() {
        let d1 = get_action_detail_definition();
        assert_eq!(d1.function.name, "get_action_detail");
        assert!(d1.function.parameters.is_some());

        let d2 = query_world_definition();
        assert_eq!(d2.function.name, "query_world");
        assert!(d2.function.parameters.is_some());

        let d3 = list_skills_definition();
        assert_eq!(d3.function.name, "list_skills");
    }
}
