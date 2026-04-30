// ============================================================================
// Chaos Generator — Sanity 混沌硬逻辑
// ============================================================================
//
// 当 Agent 理智值低于阈值时，从 available_actions 中随机选取生成 intents。
// 零硬编码：所有动作、权重、字段均来自 game_rules 数据驱动。
// ============================================================================

use cyber_jianghu_protocol::{AvailableAction, ChaosMarker, Intent, WorldState};
use rand::RngExt;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};
use uuid::Uuid;

/// 混沌配置
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
    /// 生存优先阈值（hunger/thirst 低于此值时优先选 survival category action）
    #[serde(default = "default_survival_threshold")]
    pub survival_threshold: i32,
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
fn default_survival_threshold() -> i32 {
    30
}

impl Default for ChaosConfig {
    fn default() -> Self {
        Self {
            activation_threshold: default_threshold(),
            activation_probability: default_probability(),
            max_chaos_intents: default_max(),
            survival_threshold: default_survival_threshold(),
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
        available_actions: &[AvailableAction],
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
            info!(
                "Chaos: sanity={} > threshold={}, skipping",
                sanity, self.config.activation_threshold
            );
            return Vec::new();
        }

        // 概率检查
        let mut rng = rand::rng();
        if !rng.random_bool(self.config.activation_probability) {
            return Vec::new();
        }

        let max_chaos = self.config.max_chaos_intents.min(max_total);

        // 无可用动作则无法生成 chaos intents
        if available_actions.is_empty() {
            return Vec::new();
        }

        // 优先使用 available_actions（数据驱动）
        let agent_id = world_state.agent_id.unwrap_or_default();
        let tick_id = world_state.tick_id;
        let thought = format!("[低理智混沌: sanity={}]", sanity);
        let marker = ChaosMarker::Sanity { sanity };

        let intents = Self::select_resolvable_intents(
            available_actions,
            world_state,
            agent_id,
            tick_id,
            max_chaos,
            &thought,
            Some(marker),
            &mut rng,
            self.config.survival_threshold,
        );

        info!(
            "Chaos: sanity={}, threshold={}, generated {} chaos intents from {} available actions",
            sanity,
            self.config.activation_threshold,
            intents.len(),
            available_actions.len()
        );
        intents
    }

    /// LLM 失败触发的 chaos — 不检查 sanity，100% 触发
    pub fn generate_llm_chaos_intents(
        &mut self,
        world_state: &WorldState,
        available_actions: &[AvailableAction],
        max_total: usize,
        consecutive_failures: usize,
    ) -> Vec<Intent> {
        let max_chaos = self.config.max_chaos_intents.min(max_total);
        if available_actions.is_empty() || max_chaos == 0 {
            return Vec::new();
        }

        let agent_id = world_state.agent_id.unwrap_or_default();
        let tick_id = world_state.tick_id;
        let thought = "[LLM 配额耗尽: 自动生存模式]".to_owned();
        let marker = ChaosMarker::LlmQuotaExhausted {
            consecutive_failures,
        };
        let mut rng = rand::rng();

        let intents = Self::select_resolvable_intents(
            available_actions,
            world_state,
            agent_id,
            tick_id,
            max_chaos,
            &thought,
            Some(marker),
            &mut rng,
            self.config.survival_threshold,
        );

        debug!("LLM Chaos: generated {} survival intents", intents.len());
        intents
    }

    /// 随机选取 action 并解析 required_fields，跳过字段无法满足的 action
    ///
    /// 每个槽位最多重试 `MAX_RESOLVE_RETRIES` 次以找到字段可解析的 action。
    const MAX_RESOLVE_RETRIES: usize = 6;

    #[allow(clippy::too_many_arguments)]
    fn select_resolvable_intents(
        available_actions: &[AvailableAction],
        world_state: &WorldState,
        agent_id: Uuid,
        tick_id: i64,
        max_chaos: usize,
        thought: &str,
        marker: Option<ChaosMarker>,
        rng: &mut impl rand::RngExt,
        survival_threshold: i32,
    ) -> Vec<Intent> {
        let count: usize = rng.random_range(1..=max_chaos);
        let mut intents = Vec::with_capacity(count);

        // 生存优先：hunger/thirst 低于阈值时，先从 survival category 中选取
        // survival 都不可用时 fallback 到全部 actions（数据驱动，category 来自 actions.yaml）
        let hunger = world_state
            .self_state
            .attributes
            .get("hunger")
            .copied()
            .unwrap_or(100);
        let thirst = world_state
            .self_state
            .attributes
            .get("thirst")
            .copied()
            .unwrap_or(100);

        let survival_actions: Vec<&AvailableAction> = if hunger < survival_threshold
            || thirst < survival_threshold
        {
            available_actions
                .iter()
                .filter(|a| a.category == "survival")
                .collect()
        } else {
            Vec::new()
        };

        for _ in 0..count {
            let mut resolved = false;

            // 阶段 1：优先尝试 survival actions
            if !survival_actions.is_empty() {
                for _ in 0..Self::MAX_RESOLVE_RETRIES {
                    let idx = rng.random_range(0..survival_actions.len());
                    let action = survival_actions[idx];
                    if let Some(data) = Self::build_action_data(
                        &action.action,
                        &action.required_fields,
                        world_state,
                        rng,
                    ) {
                        let mut intent =
                            Intent::new(agent_id, tick_id, action.action.as_str(), Some(data))
                                .with_thought(thought.to_owned());
                        if let Some(ref m) = marker {
                            intent = intent.with_chaos_marker(m.clone());
                        }
                        intents.push(intent);
                        resolved = true;
                        break;
                    }
                }
            }

            // 阶段 2：survival 全部失败时 fallback 到全部 actions
            if !resolved {
                for _ in 0..Self::MAX_RESOLVE_RETRIES {
                    let idx = rng.random_range(0..available_actions.len());
                    let action = &available_actions[idx];
                    if let Some(data) = Self::build_action_data(
                        &action.action,
                        &action.required_fields,
                        world_state,
                        rng,
                    ) {
                        let mut intent =
                            Intent::new(agent_id, tick_id, action.action.as_str(), Some(data))
                                .with_thought(thought.to_owned());
                        if let Some(ref m) = marker {
                            intent = intent.with_chaos_marker(m.clone());
                        }
                        intents.push(intent);
                        break;
                    } else {
                        debug!(
                            "Chaos: action '{}' skipped — required_fields unresolvable: {:?}",
                            action.action, action.required_fields
                        );
                    }
                }
            }
        }

        intents
    }

    /// 根据 required_fields 从 WorldState 动态构建 action_data
    ///
    /// 所有 required_fields 必须成功解析才返回 Some，否则返回 None。
    /// 未在 WorldState 中提供的字段（如 recipe_id）会导致整个 action 被跳过。
    fn build_action_data(
        _action_type: &str,
        required_fields: &[String],
        world_state: &WorldState,
        rng: &mut impl rand::RngExt,
    ) -> Option<serde_json::Value> {
        if required_fields.is_empty() {
            return Some(serde_json::json!({}));
        }

        let mut map = serde_json::Map::new();

        for field in required_fields {
            let resolved = match field.as_str() {
                // 目标 Agent — 从附近实体中随机选
                "target_agent_id" | "target_id" => {
                    if world_state.entities.is_empty() {
                        None
                    } else {
                        let target =
                            &world_state.entities[rng.random_range(0..world_state.entities.len())];
                        map.insert(
                            field.clone(),
                            serde_json::Value::String(target.id.to_string()),
                        );
                        Some(())
                    }
                }
                // 地面物品 — 从附近物品中随机选
                "item_id" => {
                    if world_state.nearby_items.is_empty() {
                        None
                    } else {
                        let item = &world_state.nearby_items
                            [rng.random_range(0..world_state.nearby_items.len())];
                        map.insert(
                            field.clone(),
                            serde_json::Value::String(item.item_id.clone()),
                        );
                        Some(())
                    }
                }
                // 位置节点 — 从可达节点中随机选
                "target_location" | "node_id" => {
                    if world_state.location.adjacent_nodes.is_empty() {
                        None
                    } else {
                        let node = &world_state.location.adjacent_nodes
                            [rng.random_range(0..world_state.location.adjacent_nodes.len())];
                        map.insert(
                            field.clone(),
                            serde_json::Value::String(node.node_id.clone()),
                        );
                        Some(())
                    }
                }
                // 数量 — 随机 1~3
                "quantity" => {
                    let qty: u32 = rng.random_range(1..=3);
                    map.insert(field.clone(), serde_json::Value::Number(qty.into()));
                    Some(())
                }
                // 动作内容 — 混沌状态使用 "..."
                "content" => {
                    map.insert(field.clone(), serde_json::Value::String("...".into()));
                    Some(())
                }
                _ => None,
            };
            resolved?;
        }

        Some(serde_json::Value::Object(map))
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
            aliases: vec![],
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
                    aliases: vec![],
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
                aliases: vec![],
            }],
            self_state: AgentSelfState {
                attributes: attrs,
                derived_attributes: HashMap::new(),
                attribute_descriptions: HashMap::new(),
                status_effects: vec![],
                inventory: inv,
                skills: vec![],
                age_years: None,
                max_age: None,
            },
            events_log: vec![],
            private_dialogue_log: vec![],
            last_execution_summary: None,
            lessons_learned: vec![],
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
        let actions: Vec<cyber_jianghu_protocol::AvailableAction> = vec![];
        let intents = generator.generate_chaos_intents(&ws, &actions, 5);
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
        let actions: Vec<cyber_jianghu_protocol::AvailableAction> = vec![];
        let intents = generator.generate_chaos_intents(&ws, &actions, 5);
        assert!(intents.is_empty()); // 无 available_actions 则无法生成
    }

    #[test]
    fn test_chaos_probability() {
        let config = ChaosConfig {
            activation_probability: 0.0, // 永不触发
            ..ChaosConfig::default()
        };
        let mut generator = ChaosGenerator::new(config);
        let ws = mock_world_state(5);
        let actions: Vec<cyber_jianghu_protocol::AvailableAction> = vec![];
        let intents = generator.generate_chaos_intents(&ws, &actions, 5);
        assert!(intents.is_empty());
    }
}
