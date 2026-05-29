// ============================================================================
// 工具结果紧凑化 — budget 感知的 JSON 结构精简
// ============================================================================
//
// 所有结构限制从 budget_chars 推导（budget_chars = context_window_tokens × ratio × 4）。
// 每种工具的紧凑化基于已知的 JSON 结构，使用每条目平均字符数来计算最大条目数。
// 保证输出始终是合法 JSON。

/// 对 tool 返回的 JSON 做 budget 感知的结构精简。
///
/// `budget_chars` = per_tool_limit（从 context_window_tokens 推导）。
/// 各工具的结构限制根据 budget 和每条目平均字符数动态计算。
pub fn compact_tool_result(
    tool_name: &str,
    value: &serde_json::Value,
    budget_chars: usize,
) -> serde_json::Value {
    match tool_name {
        "query_world" => compact_query_world(value, budget_chars),
        "search_memory" | "recall_archived" => compact_memory(value, budget_chars),
        "skill_view" => compact_skill(value, budget_chars),
        "get_relationship" | "list_relationships" => compact_relationship(value, budget_chars),
        _ => value.clone(),
    }
}

// 每 entity 平均字符数（id + name，无 state）
const AVG_ENTITY_CHARS: usize = 60;
// 每 event 平均字符数（tick_id + description）
const AVG_EVENT_CHARS: usize = 80;
// 每 inventory item 平均字符数（item_id + name + quantity）
const AVG_ITEM_CHARS: usize = 60;
// 每 memory entry 平均字符数（content 150 + meta 30）
const AVG_MEMORY_CHARS: usize = 180;
// 每 relationship entry 平均字符数
const AVG_REL_CHARS: usize = 100;
// 每 key_event entry 平均字符数
const AVG_KEY_EVENT_CHARS: usize = 80;

/// query_world 紧凑化
fn compact_query_world(value: &serde_json::Value, budget: usize) -> serde_json::Value {
    let mut v = value.clone();
    let section = v["section"].as_str().unwrap_or("");

    match section {
        "entities" => {
            if let Some(entities) = v["entities"].as_array_mut() {
                for e in entities.iter_mut() {
                    if let Some(obj) = e.as_object_mut() {
                        obj.remove("state");
                    }
                }
            }
            let max = (budget / AVG_ENTITY_CHARS).clamp(3, 30);
            if let Some(entities) = v["entities"].as_array_mut() {
                entities.truncate(max);
            }
        }
        "events" => {
            let max = (budget / AVG_EVENT_CHARS).clamp(3, 20);
            if let Some(events) = v["events"].as_array_mut() {
                events.truncate(max);
            }
        }
        "inventory" => {
            let max = (budget / AVG_ITEM_CHARS).clamp(5, 50);
            if let Some(items) = v["items"].as_array_mut() {
                items.truncate(max);
            }
        }
        _ => {}
    }
    v
}

/// search_memory / recall_archived 紧凑化
fn compact_memory(value: &serde_json::Value, budget: usize) -> serde_json::Value {
    let mut v = value.clone();
    if let Some(memories) = v["memories"].as_array_mut() {
        let max_count = (budget / AVG_MEMORY_CHARS).clamp(1, 10);
        memories.truncate(max_count);
        let content_limit = ((budget / max_count) as f64 * 0.8) as usize;
        for m in memories.iter_mut() {
            if let Some(content) = m.get("content").and_then(|c| c.as_str())
                && content.chars().count() > content_limit
            {
                let truncated: String = content.chars().take(content_limit).collect();
                if let Some(obj) = m.as_object_mut() {
                    obj.insert(
                        "content".into(),
                        serde_json::json!(format!("{}…[已截断]", truncated)),
                    );
                }
            }
        }
    }
    v
}

/// skill_view 紧凑化
fn compact_skill(value: &serde_json::Value, budget: usize) -> serde_json::Value {
    let mut v = value.clone();
    if let Some(content) = v.get("content").and_then(|c| c.as_str()) {
        // content 使用 budget 的 95%（skill_id 等元数据占 ~5%）
        let content_limit = (budget as f64 * 0.95) as usize;
        if content.chars().count() > content_limit {
            let truncated: String = content.chars().take(content_limit).collect();
            if let Some(obj) = v.as_object_mut() {
                obj.insert(
                    "content".into(),
                    serde_json::json!(format!("{}…\n[技能指引已截断]", truncated)),
                );
            }
        }
    }
    v
}

/// get_relationship / list_relationships 紧凑化
fn compact_relationship(value: &serde_json::Value, budget: usize) -> serde_json::Value {
    let mut v = value.clone();
    let max_events = (budget / AVG_KEY_EVENT_CHARS).clamp(3, 20);
    if let Some(events) = v
        .get_mut("relationship")
        .and_then(|r| r.get_mut("key_events"))
        .and_then(|e| e.as_array_mut())
    {
        events.truncate(max_events);
    }
    let max_rels = (budget / AVG_REL_CHARS).clamp(5, 30);
    if let Some(rels) = v.get_mut("relationships").and_then(|r| r.as_array_mut()) {
        rels.truncate(max_rels);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compact_query_world_entities_adapts_to_budget() {
        let mut entities = Vec::new();
        for i in 0..30 {
            entities.push(serde_json::json!({
                "id": format!("id-{}", i),
                "name": format!("角色{}", i),
                "state": "这是一段很长的状态描述".to_string(),
            }));
        }
        let value = serde_json::json!({
            "success": true,
            "section": "entities",
            "entities": entities,
        });

        // 小 budget (960 chars, 8K model) → max = 960/60 = 16
        let compacted = compact_tool_result("query_world", &value, 960);
        let entities = compacted["entities"].as_array().unwrap();
        assert!(entities.len() <= 16);
        assert!(entities[0].get("state").is_none());

        // 大 budget (3840 chars, 32K model) → max = 3840/60 = 30
        let compacted_large = compact_tool_result("query_world", &value, 3840);
        let entities_large = compacted_large["entities"].as_array().unwrap();
        assert!(entities_large.len() <= 30);
    }

    #[test]
    fn test_compact_query_world_events() {
        let value = serde_json::json!({
            "success": true,
            "section": "events",
            "events": (0..20).map(|i| serde_json::json!({
                "tick_id": i,
                "description": format!("事件{}", i),
            })).collect::<Vec<_>>(),
        });

        // budget=960 → max = 960/80 = 12
        let compacted = compact_tool_result("query_world", &value, 960);
        let events = compacted["events"].as_array().unwrap();
        assert!(events.len() <= 12);
    }

    #[test]
    fn test_compact_memory_adapts_to_budget() {
        let long_content: String = "这是一段很长的记忆内容，".repeat(20);
        let value = serde_json::json!({
            "success": true,
            "memories": (0..10).map(|i| serde_json::json!({
                "content": format!("{}第{}条", long_content, i),
                "tick_id": i,
                "importance": 0.5,
            })).collect::<Vec<_>>(),
        });

        // budget=960 → max_count = 960/180 = 5
        let compacted = compact_tool_result("search_memory", &value, 960);
        let memories = compacted["memories"].as_array().unwrap();
        assert!(memories.len() <= 5);
        // content 被截断
        let content = memories[0]["content"].as_str().unwrap();
        assert!(content.contains("[已截断]"));
    }

    #[test]
    fn test_compact_skill_adapts_to_budget() {
        let long_content: String = "x".repeat(5000);
        let value = serde_json::json!({
            "skill_id": "social/trust-reading",
            "content": long_content,
        });

        // budget=3840 (32K model) → content_limit = 3840 * 0.95 = 3648
        let compacted = compact_tool_result("skill_view", &value, 3840);
        let content = compacted["content"].as_str().unwrap();
        assert!(content.contains("[技能指引已截断]"));
        assert!(content.len() < 5000);
        assert_eq!(compacted["skill_id"], "social/trust-reading");

        // budget=960 (8K model) → content_limit = 960 * 0.95 = 912
        let compacted_small = compact_tool_result("skill_view", &value, 960);
        let content_small = compacted_small["content"].as_str().unwrap();
        assert!(content_small.len() < 1000);
    }

    #[test]
    fn test_compact_relationship() {
        let value = serde_json::json!({
            "success": true,
            "relationship": {
                "name": "张三",
                "favorability": 50,
                "key_events": (0..20).map(|i| serde_json::json!({
                    "tick_id": i, "action": "对话", "description": format!("事件{}", i),
                    "favorability_delta": 5,
                })).collect::<Vec<_>>(),
            },
        });

        let compacted = compact_tool_result("get_relationship", &value, 3840);
        assert!(compacted["relationship"]["key_events"].as_array().unwrap().len() <= 20);
        assert_eq!(compacted["relationship"]["name"], "张三");
    }

    #[test]
    fn test_compact_unknown_tool() {
        let value = serde_json::json!({"success": true, "data": "unchanged"});
        let compacted = compact_tool_result("unknown_tool", &value, 2000);
        assert_eq!(compacted, value);
    }

    #[test]
    fn test_compact_output_is_valid_json() {
        let value = serde_json::json!({
            "success": true,
            "section": "entities",
            "entities": (0..30).map(|i| serde_json::json!({
                "id": format!("id-{}", i),
                "name": format!("角色{}", i),
                "state": format!("状态{}", i),
            })).collect::<Vec<_>>(),
        });

        let compacted = compact_tool_result("query_world", &value, 960);
        let json_str = compacted.to_string();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert!(parsed["success"].as_bool().unwrap());
    }
}
