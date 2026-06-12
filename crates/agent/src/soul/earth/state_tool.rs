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
                    "description": "动作类型名称（如 攻击, 移动, 制造 等），必须是可用动作列表中的精确名称"
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

/// lookup_character tool 定义
pub fn lookup_character_definition() -> ToolDefinition {
    ToolDefinition::new(
        "lookup_character",
        "当你只知道角色名字、但动作需要填写 target_agent_id（UUID）时，调用此工具将名字转为 UUID。ReflectorSoul 拒绝非 UUID 格式的 target_agent_id，因此必须在提交动作前先用此工具获取精确 UUID。支持名字部分匹配。只能查到同地点的角色。",
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "你已知的角色名字。支持部分匹配，传入你知道的名字片段即可（如传'沈'可查到'沈吟'）。"
                }
            },
            "required": ["name"]
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
/// 查找顺序：精确匹配 action 字段 → 精确匹配 name 字段
/// 返回动作详情 + 基于 required_fields 动态生成的示例
pub fn execute_get_action_detail(
    action_type: &str,
    available_actions: &[AvailableAction],
) -> serde_json::Value {
    let action = available_actions
        .iter()
        .find(|a| a.action == action_type || a.name == action_type);

    match action {
        Some(a) => {
            let example = build_action_example(&a.action, &a.required_fields, &a.optional_fields);
            serde_json::json!({
                "success": true,
                "action": a.action,
                "name": a.name,
                "description": a.description,
                "category": a.category,
                "required_fields": a.required_fields,
                "optional_fields": a.optional_fields,
                "valid_targets": a.valid_targets,
                "requirements": a.requirements,
                "effects": a.effects,
                "example": example,
            })
        }
        None => serde_json::json!({
            "success": false,
            "message": format!("未找到动作: {}。请使用可用动作列表中的名称。", action_type)
        }),
    }
}

/// 根据 required_fields + optional_fields 动态生成 action 示例
///
/// 按字段组合模式匹配，生成带注释的 JSON 示例。
/// target_agent_id 字段额外标注 UUID 规则。
fn build_action_example(
    action_type: &str,
    required_fields: &[String],
    optional_fields: &[String],
) -> String {
    let all_fields: Vec<&String> = required_fields
        .iter()
        .chain(optional_fields.iter())
        .collect();
    let has_item_id = all_fields.iter().any(|f| f.as_str() == "item_id");
    let has_content = all_fields.iter().any(|f| f.as_str() == "content");
    let has_target_location = all_fields.iter().any(|f| f.as_str() == "target_location");
    let has_quantity = all_fields.iter().any(|f| f.as_str() == "quantity");
    let has_channel = all_fields.iter().any(|f| f.as_str() == "channel");
    let has_recipient_type = all_fields.iter().any(|f| f.as_str() == "recipient_type");
    let has_source_type = all_fields.iter().any(|f| f.as_str() == "source_type");
    let has_recipe_id = all_fields.iter().any(|f| f.as_str() == "recipe_id");

    let req_target_agent_id = required_fields.iter().any(|f| f == "target_agent_id");
    let opt_target_agent_id = optional_fields.iter().any(|f| f == "target_agent_id");
    let opt_recipient_id = optional_fields.iter().any(|f| f == "recipient_id");
    let opt_source_id = optional_fields.iter().any(|f| f == "source_id");

    let mut fields = Vec::new();
    if has_recipient_type {
        fields.push("\"recipient_type\": \"agent 或 ground\"".to_string());
    }
    if opt_recipient_id {
        fields.push("\"recipient_id\": \"(recipient_type=agent 时必填: 目标 UUID)\"".to_string());
    }
    if has_source_type {
        fields.push("\"source_type\": \"ground/agent/resource\"".to_string());
    }
    if opt_source_id {
        fields.push("\"source_id\": \"(source_type=agent 时必填: 来源 UUID)\"".to_string());
    }
    if req_target_agent_id {
        fields.push(
            "\"target_agent_id\": \"(必填: 先用 lookup_character 查角色的 UUID 再填入，不要直接填角色名字)\""
                .to_string(),
        );
    } else if opt_target_agent_id {
        fields.push(
            "\"target_agent_id\": \"(可选: 向特定人物说话/观察特定角色时填入其 UUID)\"".to_string(),
        );
    }
    if has_channel {
        fields.push("\"channel\": \"(可选: public/private/broadcast)\"".to_string());
    }
    if has_target_location {
        fields.push("\"target_location\": \"(从可前往的地点列表复制)\"".to_string());
    }
    if has_recipe_id {
        fields.push("\"recipe_id\": \"(配方 ID)\"".to_string());
    }
    if has_item_id {
        fields.push("\"item_id\": \"(从背包或附近物品列表复制)\"".to_string());
    }
    if has_quantity {
        fields.push("\"quantity\": 1".to_string());
    }
    if has_content {
        fields.push("\"content\": \"...\"".to_string());
    }

    let fields_str = if fields.is_empty() {
        String::new()
    } else {
        fields.join(", ")
    };

    format!(
        "{{\"actions\": [{{\"action_type\": \"{}\", \"action_data\": {{{}}}}}]}}",
        action_type, fields_str
    )
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

/// 执行 lookup_character
///
/// 根据角色名称在附近实体中查找匹配项，返回 UUID 字符串。
pub async fn execute_lookup_character(name: &str, store: &WorldStateStore) -> serde_json::Value {
    let ws = match store.current().await {
        Some(ws) => ws,
        None => {
            return serde_json::json!({
                "success": false,
                "message": "WorldState 尚未初始化"
            });
        }
    };

    let matches: Vec<_> = ws
        .entities
        .iter()
        .filter(|e| e.name.contains(name))
        .map(|e| {
            serde_json::json!({
                "name": e.name,
                "id": e.id.to_string(),
                "state": e.state,
                "distance": e.distance,
                "hostile": e.hostile,
            })
        })
        .collect();

    let hint = if matches.is_empty() {
        "未找到匹配角色，请检查名字是否正确，或使用 query_world(section: 'entities') 查看附近所有角色".to_string()
    } else if matches.len() == 1 {
        format!(
            "找到角色 '{}'，UUID: {}",
            matches[0]["name"], matches[0]["id"]
        )
    } else {
        "找到多个匹配角色，请使用更完整的名称重新查询".to_string()
    };

    serde_json::json!({
        "success": true,
        "matches": matches,
        "total": matches.len(),
        "hint": hint,
    })
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

    fn make_action(action: &str, name: &str) -> AvailableAction {
        AvailableAction {
            action: action.to_string(),
            name: name.to_string(),
            description: format!("{}描述", name),
            category: "test".to_string(),
            valid_targets: None,
            required_fields: vec![],
            optional_fields: vec![],
            ooc_risk: "low".to_string(),
            requirements: vec![],
            effects: vec![],
        }
    }

    #[test]
    fn test_get_action_detail_exact_match() {
        let actions = vec![make_action("attack", "攻击"), make_action("qu", "取")];
        let result = execute_get_action_detail("attack", &actions);
        assert!(result["success"].as_bool().unwrap());
        assert_eq!(result["action"], "attack");
        assert_eq!(result["name"], "攻击");
    }

    #[test]
    fn test_get_action_detail_name_match() {
        let actions = vec![make_action("attack", "攻击")];
        let result = execute_get_action_detail("攻击", &actions);
        assert!(result["success"].as_bool().unwrap());
        assert_eq!(result["action"], "attack");
    }

    #[test]
    fn test_get_action_detail_not_found() {
        let actions = vec![make_action("attack", "攻击")];
        let result = execute_get_action_detail("nonexistent", &actions);
        assert!(!result["success"].as_bool().unwrap());
        assert!(result["message"].as_str().unwrap().contains("nonexistent"));
    }

    #[test]
    fn test_get_action_detail_returns_optional_fields() {
        let speak = AvailableAction {
            action: "说话".to_string(),
            name: "交谈".to_string(),
            description: "发出信息".to_string(),
            category: "social".to_string(),
            valid_targets: None,
            required_fields: vec!["content".to_string()],
            optional_fields: vec!["channel".to_string(), "target_agent_id".to_string()],
            ooc_risk: "high".to_string(),
            requirements: vec![],
            effects: vec![],
        };
        let result = execute_get_action_detail("说话", &[speak]);
        assert!(result["success"].as_bool().unwrap());
        let opt = result["optional_fields"].as_array().unwrap();
        assert_eq!(opt.len(), 2);
        assert_eq!(opt[0], "channel");
        assert_eq!(opt[1], "target_agent_id");
        let example = result["example"].as_str().unwrap();
        assert!(example.contains("target_agent_id"));
        assert!(example.contains("channel"));
    }

    #[test]
    fn test_get_action_detail_yu_with_recipient_id() {
        let yu = AvailableAction {
            action: "予".to_string(),
            name: "予".to_string(),
            description: "给予".to_string(),
            category: "survival".to_string(),
            valid_targets: None,
            required_fields: vec![
                "recipient_type".to_string(),
                "item_id".to_string(),
                "quantity".to_string(),
            ],
            optional_fields: vec!["recipient_id".to_string()],
            ooc_risk: "low".to_string(),
            requirements: vec![],
            effects: vec![],
        };
        let result = execute_get_action_detail("予", &[yu]);
        let example = result["example"].as_str().unwrap();
        assert!(example.contains("recipient_type"));
        assert!(example.contains("recipient_id"));
        assert!(example.contains("item_id"));
    }

    #[test]
    fn test_get_action_detail_qu_with_source_id() {
        let qu = AvailableAction {
            action: "取".to_string(),
            name: "取".to_string(),
            description: "获取".to_string(),
            category: "survival".to_string(),
            valid_targets: None,
            required_fields: vec![
                "source_type".to_string(),
                "item_id".to_string(),
                "quantity".to_string(),
            ],
            optional_fields: vec!["source_id".to_string()],
            ooc_risk: "medium".to_string(),
            requirements: vec![],
            effects: vec![],
        };
        let result = execute_get_action_detail("取", &[qu]);
        let example = result["example"].as_str().unwrap();
        assert!(example.contains("source_type"));
        assert!(example.contains("source_id"));
    }

    #[test]
    fn test_get_action_detail_craft_with_recipe_id() {
        let craft = AvailableAction {
            action: "制造".to_string(),
            name: "锻造".to_string(),
            description: "制造物品".to_string(),
            category: "economic".to_string(),
            valid_targets: None,
            required_fields: vec!["recipe_id".to_string()],
            optional_fields: vec![],
            ooc_risk: "low".to_string(),
            requirements: vec![],
            effects: vec![],
        };
        let result = execute_get_action_detail("制造", &[craft]);
        let example = result["example"].as_str().unwrap();
        assert!(example.contains("recipe_id"));
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
                survival_drives: vec![],
                status_effects: vec!["健康".to_string()],
                inventory: vec![cyber_jianghu_protocol::InventoryItem {
                    item_id: "mantou".to_string(),
                    name: "馒头".to_string(),
                    quantity: 3,
                    is_equipped: false,
                    item_type: "consumable".to_string(),
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

    // ---- lookup_character ----

    #[tokio::test]
    async fn test_lookup_character_exact_match() {
        let store = WorldStateStore::new();
        store.update(make_test_world_state()).await;
        let result = execute_lookup_character("路人甲", &store).await;
        assert!(result["success"].as_bool().unwrap());
        assert_eq!(result["total"], 1);
        assert_eq!(result["matches"][0]["name"], "路人甲");
        assert!(!result["matches"][0]["id"].as_str().unwrap().is_empty());
        assert!(
            uuid::Uuid::parse_str(result["matches"][0]["id"].as_str().unwrap()).is_ok(),
            "返回的 id 应为合法 UUID"
        );
    }

    #[tokio::test]
    async fn test_lookup_character_partial_match() {
        let store = WorldStateStore::new();
        store.update(make_test_world_state()).await;
        let result = execute_lookup_character("路人", &store).await;
        assert!(result["success"].as_bool().unwrap());
        assert_eq!(result["total"], 1);
    }

    #[tokio::test]
    async fn test_lookup_character_no_match() {
        let store = WorldStateStore::new();
        store.update(make_test_world_state()).await;
        let result = execute_lookup_character("不存在", &store).await;
        assert!(result["success"].as_bool().unwrap());
        assert_eq!(result["total"], 0);
        assert!(result["hint"].as_str().unwrap().contains("未找到"));
    }

    #[tokio::test]
    async fn test_lookup_character_no_world_state() {
        let store = WorldStateStore::new();
        let result = execute_lookup_character("路人甲", &store).await;
        assert!(!result["success"].as_bool().unwrap());
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
