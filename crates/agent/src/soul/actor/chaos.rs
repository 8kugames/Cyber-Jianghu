// ============================================================================
// Chaos Generator — Sanity 混沌硬逻辑
// ============================================================================
//
// 当 Agent 理智值低于阈值时，代码生成随机 intents（仍经 ReflectorSoul 审核）。
// 与 LLM 正常 intent 合并为 multi-intent pipeline。
// ============================================================================

use cyber_jianghu_protocol::{Intent, WorldState};
use rand::RngExt;
use rand::distr::Distribution;
use rand::distr::weighted::WeightedIndex;
use serde::{Deserialize, Serialize};
use tracing::debug;

/// 混沌配置（使用硬编码默认值，未来可从 YAML 配置加载）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChaosConfig {
    /// 触发阈值（sanity <= 此值时激活）
    #[serde(default = "default_threshold")]
    pub activation_threshold: i32,
    /// 触发概率（0.0-1.0）
    #[serde(default = "default_probability")]
    pub activation_probability: f64,
    /// 最大混沌 intent 数
    #[serde(default = "default_max")]
    pub max_chaos_intents: usize,
    /// 动作权重（action_type → weight）
    #[serde(default = "default_action_weights")]
    pub action_weights: std::collections::HashMap<String, f64>,
}

fn default_threshold() -> i32 {
    30
}
fn default_probability() -> f64 {
    0.5
}
fn default_max() -> usize {
    3
}
fn default_action_weights() -> std::collections::HashMap<String, f64> {
    let mut m = std::collections::HashMap::new();
    m.insert("attack".into(), 0.25);
    m.insert("drop".into(), 0.20);
    m.insert("give".into(), 0.15);
    m.insert("move".into(), 0.20);
    m.insert("eat".into(), 0.10);
    m.insert("drink".into(), 0.10);
    m
}

impl Default for ChaosConfig {
    fn default() -> Self {
        Self {
            activation_threshold: default_threshold(),
            activation_probability: default_probability(),
            max_chaos_intents: default_max(),
            action_weights: default_action_weights(),
        }
    }
}

/// 混沌意图生成器
pub struct ChaosGenerator {
    config: ChaosConfig,
}

impl ChaosGenerator {
    /// 创建混沌生成器
    pub fn new(config: ChaosConfig) -> Self {
        Self { config }
    }

    /// 检查是否触发混沌，并生成随机 intents
    ///
    /// 返回空 Vec 表示未触发或无可用动作。
    pub fn generate_chaos_intents(
        &mut self,
        world_state: &WorldState,
        max_total: usize,
    ) -> Vec<Intent> {
        let sanity = world_state
            .self_state
            .attributes
            .get("sanity")
            .copied()
            .unwrap_or(100);

        // 阈值检查
        if sanity > self.config.activation_threshold {
            return Vec::new();
        }

        // 概率检查
        let mut rng = rand::rng();
        if !rng.random_bool(self.config.activation_probability) {
            return Vec::new();
        }

        let max_chaos = self.config.max_chaos_intents.min(max_total);
        let action_types: Vec<&String> = self.config.action_weights.keys().collect();

        if action_types.is_empty() {
            return Vec::new();
        }

        // 构建加权选择
        let weights: Vec<f64> = action_types
            .iter()
            .map(|at| *self.config.action_weights.get(*at).unwrap_or(&0.1))
            .collect();

        let dist = match WeightedIndex::new(&weights) {
            Ok(d) => d,
            Err(_) => return Vec::new(),
        };

        let agent_id = world_state.agent_id.unwrap_or_default();
        let tick_id = world_state.tick_id;
        let mut intents = Vec::new();

        let count: usize = rng.random_range(1..=max_chaos);
        for _ in 0..count {
            let idx = dist.sample(&mut rng);
            let action_type = action_types[idx].as_str();
            let action_data = Self::build_action_data(action_type, world_state, &mut rng);

            intents.push(
                Intent::new(agent_id, tick_id, action_type, action_data)
                    .with_thought(format!("[混沌行为: sanity={}]", sanity)),
            );
        }

        debug!(
            "Chaos: sanity={}, generated {} chaos intents",
            sanity,
            intents.len()
        );
        intents
    }

    /// 构建动作数据（基于 WorldState 中可用的实体）
    fn build_action_data(
        action_type: &str,
        world_state: &WorldState,
        rng: &mut impl rand::RngExt,
    ) -> Option<serde_json::Value> {
        match action_type {
            "attack" => {
                // 攻击附近的随机实体
                if world_state.entities.is_empty() {
                    return None;
                }
                let target = &world_state.entities[rng.random_range(0..world_state.entities.len())];
                Some(serde_json::json!({
                    "target_id": target.id.to_string(),
                }))
            }
            "drop" => {
                // 丢弃背包中的随机物品
                if world_state.self_state.inventory.is_empty() {
                    return None;
                }
                let items = &world_state.self_state.inventory;
                let item = &items[rng.random_range(0..items.len())];
                Some(serde_json::json!({
                    "item_id": item.item_id,
                    "quantity": 1,
                }))
            }
            "give" => {
                // 给附近随机实体随机物品
                if world_state.entities.is_empty() || world_state.self_state.inventory.is_empty() {
                    return None;
                }
                let target = &world_state.entities[rng.random_range(0..world_state.entities.len())];
                let items = &world_state.self_state.inventory;
                let item = &items[rng.random_range(0..items.len())];
                Some(serde_json::json!({
                    "target_id": target.id.to_string(),
                    "item_id": item.item_id,
                    "quantity": 1,
                }))
            }
            "move" => {
                // 移动到随机可达地点
                if world_state.location.adjacent_nodes.is_empty() {
                    return None;
                }
                let nodes = &world_state.location.adjacent_nodes;
                let node = &nodes[rng.random_range(0..nodes.len())];
                Some(serde_json::json!({
                    "target_location": node.node_id,
                }))
            }
            "eat" | "drink" => {
                // 随机吃/喝背包中的物品
                if world_state.self_state.inventory.is_empty() {
                    return None;
                }
                let items = &world_state.self_state.inventory;
                let item = &items[rng.random_range(0..items.len())];
                Some(serde_json::json!({
                    "item_id": item.item_id,
                }))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_jianghu_protocol::{
        AdjacentNode, AgentSelfState, Entity, Location, SceneItem, WorldTime,
    };
    use std::collections::HashMap;

    fn mock_world_state(sanity: i32) -> WorldState {
        let mut attrs = HashMap::new();
        attrs.insert("sanity".into(), sanity);
        attrs.insert("hunger".into(), 50);
        attrs.insert("thirst".into(), 50);

        let inv = vec![cyber_jianghu_protocol::InventoryItem {
            item_id: "test_item".into(),
            name: "测试物品".into(),
            item_type: "food".into(),
            quantity: 1,
            is_equipped: false,
        }];

        WorldState {
            event_type: "world_state".into(),
            tick_id: 100,
            agent_id: Some(uuid::Uuid::new_v4()),
            location: Location {
                node_id: "loc_a".into(),
                name: "地点A".into(),
                node_type: "inn".into(),
                adjacent_nodes: vec![AdjacentNode {
                    node_id: "loc_b".into(),
                    name: "地点B".into(),
                    travel_cost: 1,
                }],
                gatherable_items: vec![],
            },
            entities: vec![Entity {
                id: uuid::Uuid::new_v4(),
                name: "NPC1".into(),
                distance: 0,
                state: "alive".into(),
                hostile: false,
                recent_actions: vec![],
            }],
            nearby_items: vec![SceneItem {
                item_id: "ground_item".into(),
                name: "地面物品".into(),
                item_type: "food".into(),
                quantity: 1,
            }],
            self_state: AgentSelfState {
                attributes: attrs,
                derived_attributes: HashMap::new(),
                attribute_descriptions: HashMap::new(),
                status_effects: vec![],
                inventory: inv,
            },
            events_log: vec![],
            private_dialogue_log: vec![],
            last_execution_summary: None,
            world_time: WorldTime {
                year: 1,
                month: 1,
                day: 1,
                hour: 8,
                minute: 0,
                second: 0,
                weather: "晴".into(),
            },
        }
    }

    #[test]
    fn test_no_chaos_above_threshold() {
        let mut generator = ChaosGenerator::new(ChaosConfig::default());
        let ws = mock_world_state(80);
        let intents = generator.generate_chaos_intents(&ws, 5);
        assert!(intents.is_empty());
    }

    #[test]
    fn test_chaos_below_threshold() {
        let config = ChaosConfig {
            activation_probability: 1.0, // 强制触发
            ..ChaosConfig::default()
        };
        let mut generator = ChaosGenerator::new(config);
        let ws = mock_world_state(10);
        let intents = generator.generate_chaos_intents(&ws, 5);
        assert!(!intents.is_empty());
        // 每个 intent 都带 thought_process
        for intent in &intents {
            assert!(intent.thought_log.as_ref().unwrap().contains("混沌"));
        }
    }

    #[test]
    fn test_chaos_probability() {
        let config = ChaosConfig {
            activation_probability: 0.0, // 永不触发
            ..ChaosConfig::default()
        };
        let mut generator = ChaosGenerator::new(config);
        let ws = mock_world_state(5);
        let intents = generator.generate_chaos_intents(&ws, 5);
        assert!(intents.is_empty());
    }
}
