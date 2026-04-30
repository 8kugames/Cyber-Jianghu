// ============================================================================
// 关系工具定义与执行逻辑
// ============================================================================

use crate::component::llm::tool_types::ToolDefinition;
use crate::component::social::RelationshipStore;
use uuid::Uuid;

/// get_relationship tool 定义
pub fn get_relationship_definition() -> ToolDefinition {
    ToolDefinition::new(
        "get_relationship",
        "查询你与某个角色的关系记忆。输入角色的 UUID 或名字。",
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "identifier": {
                    "type": "string",
                    "description": "角色的 UUID 或名字"
                }
            },
            "required": ["identifier"]
        })),
    )
}

/// list_relationships tool 定义
pub fn list_relationships_definition() -> ToolDefinition {
    ToolDefinition::new(
        "list_relationships",
        "列出你认识的所有角色的关系概览。可选按好感度范围过滤。",
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "min_favorability": {
                    "type": "integer",
                    "description": "最低好感度（可选）",
                    "minimum": -100,
                    "maximum": 100
                },
                "max_favorability": {
                    "type": "integer",
                    "description": "最高好感度（可选）",
                    "minimum": -100,
                    "maximum": 100
                }
            }
        })),
    )
}

/// record_social_event tool 定义
pub fn record_social_event_definition() -> ToolDefinition {
    ToolDefinition::new(
        "record_social_event",
        "主动记录一次社交互动和好感度变化。当你经历了重要的社交事件时使用。",
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "target_agent_id": {
                    "type": "string",
                    "description": "对方的 UUID"
                },
                "target_name": {
                    "type": "string",
                    "description": "对方的名字"
                },
                "tick_id": {
                    "type": "integer",
                    "description": "当前 tick"
                },
                "action": {
                    "type": "string",
                    "description": "事件类型，如：对话、交易、给予、攻击等"
                },
                "description": {
                    "type": "string",
                    "description": "事件描述"
                },
                "favorability_delta": {
                    "type": "integer",
                    "description": "好感度变化（-50 到 50）",
                    "minimum": -50,
                    "maximum": 50
                }
            },
            "required": ["target_agent_id", "target_name", "tick_id", "action", "description", "favorability_delta"]
        })),
    )
}

/// 执行 get_relationship
pub(super) fn execute_get_relationship(
    store: &RelationshipStore,
    identifier: &str,
) -> serde_json::Value {
    // 先尝试 UUID 解析，失败则按名称查找
    if let Ok(uuid) = Uuid::parse_str(identifier) {
        match store.get_relationship(uuid) {
            Ok(Some(rel)) => relationship_to_json(&rel),
            Ok(None) => serde_json::json!({
                "success": true,
                "message": format!("未找到与 {} 的关系记录", identifier),
                "relationship": null
            }),
            Err(e) => serde_json::json!({
                "success": false,
                "error": format!("查询关系失败: {}", e)
            }),
        }
    } else {
        match store.find_relationship(identifier) {
            Ok(Some(rel)) => relationship_to_json(&rel),
            Ok(None) => serde_json::json!({
                "success": true,
                "message": format!("未找到名为「{}」的角色关系", identifier),
                "relationship": null
            }),
            Err(e) => serde_json::json!({
                "success": false,
                "error": format!("按名称查找关系失败: {}", e)
            }),
        }
    }
}

/// 执行 list_relationships
pub(super) fn execute_list_relationships(
    store: &RelationshipStore,
    min_favorability: Option<i32>,
    max_favorability: Option<i32>,
) -> serde_json::Value {
    match store.list_relationships_filtered(min_favorability, max_favorability) {
        Ok(relationships) => {
            if relationships.is_empty() {
                return serde_json::json!({
                    "success": true,
                    "message": "没有匹配的关系记录",
                    "relationships": []
                });
            }
            let entries: Vec<serde_json::Value> = relationships
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "name": r.target_name,
                        "agent_id": r.target_agent_id.to_string(),
                        "favorability": r.favorability,
                        "last_interaction_tick": r.last_interaction_tick,
                        "key_events_count": r.key_events.len()
                    })
                })
                .collect();
            serde_json::json!({
                "success": true,
                "message": format!("找到 {} 条关系记录", entries.len()),
                "relationships": entries
            })
        }
        Err(e) => serde_json::json!({
            "success": false,
            "error": format!("查询关系列表失败: {}", e)
        }),
    }
}

/// 执行 record_social_event
pub(super) fn execute_record_social_event(
    store: &RelationshipStore,
    target_agent_id: &str,
    target_name: &str,
    tick_id: i64,
    action: &str,
    description: &str,
    favorability_delta: i32,
) -> serde_json::Value {
    let uuid = match Uuid::parse_str(target_agent_id) {
        Ok(u) => u,
        Err(e) => {
            return serde_json::json!({
                "success": false,
                "error": format!("无效的 UUID: {}", e)
            });
        }
    };

    // clamp delta 到安全范围
    let clamped_delta = favorability_delta.clamp(-50, 50);

    match store.record_social_event(
        uuid,
        target_name,
        tick_id,
        action,
        description,
        clamped_delta,
    ) {
        Ok(()) => serde_json::json!({
            "success": true,
            "message": format!("已记录与「{}」的社交事件：{}", target_name, action)
        }),
        Err(e) => serde_json::json!({
            "success": false,
            "error": format!("记录社交事件失败: {}", e)
        }),
    }
}

/// 将 RelationshipMemory 转换为 JSON 响应
fn relationship_to_json(rel: &crate::component::social::RelationshipMemory) -> serde_json::Value {
    let events: Vec<serde_json::Value> = rel
        .key_events
        .iter()
        .map(|e| {
            serde_json::json!({
                "tick_id": e.tick_id,
                "action": e.event_type,
                "description": e.description,
                "favorability_delta": e.favorability_delta
            })
        })
        .collect();

    serde_json::json!({
        "success": true,
        "relationship": {
            "name": rel.target_name,
            "agent_id": rel.target_agent_id.to_string(),
            "favorability": rel.favorability,
            "last_interaction_tick": rel.last_interaction_tick,
            "key_events": events
        }
    })
}
