// ============================================================================
// ActorSoul 工具定义 + ToolExecutor（遗留，供参考）
// ============================================================================
//
// 三魂架构下，人魂（ActorSoul）不再使用 tool calling 查询精确 ID。
// 精确 ID 查询由天魂（IntentTranslator）在翻译阶段通过 prompt 内嵌的
// inventory/locations 信息完成。
//
// 保留此模块仅供参考和未来可能的工具扩展场景。

use anyhow::Result;
use async_trait::async_trait;
use cyber_jianghu_protocol::WorldState;

use crate::component::llm::tool_types::{ToolDefinition, ToolExecutor};

/// 创建 ActorSoul 使用的工具定义列表
pub fn create_actor_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::simple(
            "get_inventory",
            "查询当前背包中所有物品的精确 item_id、名称和数量。\
             在执行 eat、drink、use、drop 等涉及物品的 action 前，\
             务必调用此工具获取正确的 item_id。",
        ),
        ToolDefinition::simple(
            "get_adjacent_locations",
            "查询当前可达的地点 node_id、名称和移动耗时。\
             在执行 move action 前，务必调用此工具获取正确的 target_location。",
        ),
    ]
}

/// ActorSoul 工具执行器，基于 WorldState 数据
pub struct ActorToolExecutor {
    world_state: WorldState,
}

impl ActorToolExecutor {
    pub fn new(world_state: WorldState) -> Self {
        Self { world_state }
    }
}

#[async_trait]
impl ToolExecutor for ActorToolExecutor {
    async fn execute(
        &self,
        name: &str,
        _arguments: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        match name {
            "get_inventory" => {
                let items: Vec<_> = self
                    .world_state
                    .self_state
                    .inventory
                    .iter()
                    .map(|i| {
                        serde_json::json!({
                            "item_id": i.item_id,
                            "name": i.name,
                            "quantity": i.quantity
                        })
                    })
                    .collect();
                Ok(serde_json::json!({ "items": items }))
            }
            "get_adjacent_locations" => {
                let nodes: Vec<_> = self
                    .world_state
                    .location
                    .adjacent_nodes
                    .iter()
                    .map(|n| {
                        serde_json::json!({
                            "node_id": n.node_id,
                            "name": n.name,
                            "travel_cost": n.travel_cost
                        })
                    })
                    .collect();
                Ok(serde_json::json!({ "locations": nodes }))
            }
            _ => Err(anyhow::anyhow!("Unknown tool: {}", name)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_jianghu_protocol::{
        AdjacentNode, AgentSelfState, InventoryItem, Location, WorldTime,
    };
    use std::collections::HashMap;

    fn test_world_state() -> WorldState {
        WorldState {
            event_type: "world_state".to_string(),
            tick_id: 1,
            agent_id: None,
            world_time: WorldTime {
                year: 1,
                month: 1,
                day: 1,
                hour: 12,
                minute: 0,
                second: 0,
                weather: "晴".to_string(),
            },
            location: Location {
                node_id: "village_center".to_string(),
                name: "村口".to_string(),
                node_type: "village".to_string(),
                adjacent_nodes: vec![
                    AdjacentNode {
                        node_id: "tavern".to_string(),
                        name: "客栈".to_string(),
                        travel_cost: 1,
                    },
                    AdjacentNode {
                        node_id: "market".to_string(),
                        name: "集市".to_string(),
                        travel_cost: 2,
                    },
                ],
            },
            self_state: AgentSelfState {
                attributes: HashMap::new(),
                derived_attributes: HashMap::new(),
                attribute_descriptions: HashMap::new(),
                status_effects: vec![],
                inventory: vec![
                    InventoryItem {
                        item_id: "mantou".to_string(),
                        name: "馒头".to_string(),
                        quantity: 3,
                        is_equipped: false,
                    },
                    InventoryItem {
                        item_id: "shui_dai".to_string(),
                        name: "水袋".to_string(),
                        quantity: 1,
                        is_equipped: false,
                    },
                ],
            },
            entities: vec![],
            nearby_items: vec![],
            events_log: vec![],
            private_dialogue_log: vec![],
            deadline_ms: 0,
            last_execution_summary: None,
        }
    }

    #[tokio::test]
    async fn test_get_inventory() {
        let ws = test_world_state();
        let executor = ActorToolExecutor::new(ws);
        let result = executor
            .execute("get_inventory", &serde_json::json!({}))
            .await
            .unwrap();

        let items = result["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["item_id"], "mantou");
        assert_eq!(items[0]["name"], "馒头");
        assert_eq!(items[0]["quantity"], 3);
        assert_eq!(items[1]["item_id"], "shui_dai");
    }

    #[tokio::test]
    async fn test_get_adjacent_locations() {
        let ws = test_world_state();
        let executor = ActorToolExecutor::new(ws);
        let result = executor
            .execute("get_adjacent_locations", &serde_json::json!({}))
            .await
            .unwrap();

        let locations = result["locations"].as_array().unwrap();
        assert_eq!(locations.len(), 2);
        assert_eq!(locations[0]["node_id"], "tavern");
        assert_eq!(locations[0]["name"], "客栈");
        assert_eq!(locations[0]["travel_cost"], 1);
        assert_eq!(locations[1]["node_id"], "market");
    }

    #[tokio::test]
    async fn test_unknown_tool() {
        let ws = test_world_state();
        let executor = ActorToolExecutor::new(ws);
        let result = executor
            .execute("nonexistent", &serde_json::json!({}))
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_create_actor_tools() {
        let tools = create_actor_tools();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].function.name, "get_inventory");
        assert_eq!(tools[1].function.name, "get_adjacent_locations");
    }
}
