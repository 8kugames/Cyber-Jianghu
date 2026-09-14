// ============================================================================
// Chaos Generator — Sanity 混沌硬逻辑
// ============================================================================
//
// 当 Agent 理智值低于阈值时，从 available_actions 中随机选取生成 intents。
// 零硬编码：所有动作、权重、字段均来自 game_rules 数据驱动。
// ============================================================================

use crate::soul::item_source::{ItemActionSource, classify_item_action};
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
    /// 生存优先阈值（satiation/hydration 低于此值时优先选 survival category action）
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

        // 生存优先：satiation/hydration 低于阈值时，先从 survival category 中选取
        // survival 都不可用时 fallback 到全部 actions（数据驱动，category 来自 actions.yaml）
        let satiation = world_state
            .self_state
            .attributes
            .get("satiation")
            .copied()
            .unwrap_or(100);
        let hydration = world_state
            .self_state
            .attributes
            .get("hydration")
            .copied()
            .unwrap_or(100);

        let survival_actions: Vec<&AvailableAction> =
            if satiation < survival_threshold || hydration < survival_threshold {
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
                    let source = classify_item_action(&action.action, available_actions);
                    if let Some(data) =
                        Self::build_action_data(source, &action.required_fields, world_state, rng)
                    {
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
                    let source = classify_item_action(&action.action, available_actions);
                    if let Some(data) =
                        Self::build_action_data(source, &action.required_fields, world_state, rng)
                    {
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
    ///
    /// 物品目标按动作来源分类解析（soul/item_source）：消耗/转出类（用/吃/喝/予）
    /// 从背包取——地面物品在服务端必因背包无货回滚，不得作为 chaos 生存意图；
    /// 采集/拾取类（取）从地面 ∪ 资源点取，source_type 跟随物品实际来源。
    fn build_action_data(
        source: ItemActionSource,
        required_fields: &[String],
        world_state: &WorldState,
        rng: &mut impl rand::RngExt,
    ) -> Option<serde_json::Value> {
        if required_fields.is_empty() {
            return Some(serde_json::json!({}));
        }

        // 物品目标预解析（item_id / source_type / quantity 三字段联动：
        // 取 的 source_type 须与物品实际来源一致才能通过服务端校验）
        let picked = if required_fields.iter().any(|f| f == "item_id") {
            Self::pick_item(source, world_state, rng)
        } else {
            None
        };

        // 接收方预解析（予 的 recipient_type / recipient_id 联动：
        // chaos 行为语义 — 随机给附近任意角色，或丢在地上）
        let recipient = if required_fields.iter().any(|f| f == "recipient_type") {
            Some(Self::pick_recipient(world_state, rng))
        } else {
            None
        };

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
                // 物品 — 按动作来源分类（见函数注释）
                "item_id" => {
                    let item = picked.as_ref()?;
                    map.insert(
                        field.clone(),
                        serde_json::Value::String(item.item_id.clone()),
                    );
                    Some(())
                }
                // 物品来源 — 跟随预解析物品的实际来源（地面/资源点）
                "source_type" => match picked.as_ref().map(|p| p.origin) {
                    Some(origin @ ("ground" | "resource")) => {
                        map.insert(field.clone(), serde_json::Value::String(origin.to_string()));
                        Some(())
                    }
                    _ => None,
                },
                // 接收方类型 — 予 的 chaos 行为：随机给附近任意角色或丢在地上
                "recipient_type" => {
                    let r = recipient.as_ref()?;
                    map.insert(
                        field.clone(),
                        serde_json::Value::String(r.recipient_type.to_string()),
                    );
                    // 予-agent：recipient_id 虽列为 optional 字段，但天魂 layer0
                    // 与服务端对 agent 分支均按必填校验，须随类型一并写入
                    if let Some(id) = &r.recipient_id {
                        map.insert(
                            "recipient_id".to_string(),
                            serde_json::Value::String(id.clone()),
                        );
                    }
                    Some(())
                }
                // 接收方角色 — 从预解析结果取（若未来动作将其列为必填）
                "recipient_id" => {
                    let id = recipient.as_ref()?.recipient_id.as_ref()?;
                    map.insert(field.clone(), serde_json::Value::String(id.clone()));
                    Some(())
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
                // 数量 — 随机 1~3，以物品已知存量封顶（超量在服务端按量扣减必败）
                "quantity" => {
                    let cap = picked
                        .as_ref()
                        .map(|p| p.available_qty.min(3))
                        .unwrap_or(3)
                        .max(1);
                    let qty: u32 = rng.random_range(1..=cap);
                    map.insert(field.clone(), serde_json::Value::Number(qty.into()));
                    Some(())
                }
                // 动作内容 — 混沌状态无法生成有意义文本，跳过含此字段的 action
                "content" => None,
                // 配方 — 从已知配方中随机选
                "recipe_id" => {
                    if world_state.self_state.recipe_details.is_empty() {
                        None
                    } else {
                        let recipes = &world_state.self_state.recipe_details;
                        let idx = rng.random_range(0..recipes.len());
                        map.insert(
                            field.clone(),
                            serde_json::Value::String(recipes[idx].recipe_id.clone()),
                        );
                        Some(())
                    }
                }
                _ => None,
            };
            resolved?;
        }

        Some(serde_json::Value::Object(map))
    }

    /// 预解析物品目标：按动作来源分类选取并携带来源/存量信息
    fn pick_item(
        source: ItemActionSource,
        world_state: &WorldState,
        rng: &mut impl rand::RngExt,
    ) -> Option<PickedItem> {
        match source {
            ItemActionSource::Inventory => {
                // 消耗/转出类：物品必须来自背包；空背包返回 None（跳过该动作，
                // 避免产出引用地面物品的必败意图）
                let inventory = &world_state.self_state.inventory;
                if inventory.is_empty() {
                    return None;
                }
                let item = &inventory[rng.random_range(0..inventory.len())];
                Some(PickedItem {
                    item_id: item.item_id.clone(),
                    origin: "inventory",
                    available_qty: item.quantity.max(1) as u32,
                })
            }
            ItemActionSource::World => {
                // 采集/拾取类：地面物品 ∪ 本地点资源点（来源标注 ground/resource，
                // 资源点无数量语义，由 chaos 上限 3 自然封顶）
                let mut candidates: Vec<PickedItem> = world_state
                    .nearby_items
                    .iter()
                    .map(|i| PickedItem {
                        item_id: i.item_id.clone(),
                        origin: "ground",
                        available_qty: i.quantity.max(1) as u32,
                    })
                    .collect();
                candidates.extend(world_state.location.gatherable_items.iter().map(|g| {
                    PickedItem {
                        item_id: g.item_id.clone(),
                        origin: "resource",
                        available_qty: u32::MAX,
                    }
                }));
                if candidates.is_empty() {
                    return None;
                }
                Some(candidates.swap_remove(rng.random_range(0..candidates.len())))
            }
            ItemActionSource::Unknown => {
                // 分类缺失：维持历史行为（地面物品）
                let nearby = &world_state.nearby_items;
                if nearby.is_empty() {
                    return None;
                }
                let item = &nearby[rng.random_range(0..nearby.len())];
                Some(PickedItem {
                    item_id: item.item_id.clone(),
                    origin: "ground",
                    available_qty: item.quantity.max(1) as u32,
                })
            }
        }
    }

    /// 预解析予 的接收方：随机给附近任意角色，或丢在地上（chaos 行为语义）；
    /// 附近无人时只能丢在地上
    fn pick_recipient(world_state: &WorldState, rng: &mut impl rand::RngExt) -> PickedRecipient {
        if world_state.entities.is_empty() || rng.random_bool(0.5) {
            PickedRecipient {
                recipient_type: "ground",
                recipient_id: None,
            }
        } else {
            let target = &world_state.entities[rng.random_range(0..world_state.entities.len())];
            PickedRecipient {
                recipient_type: "agent",
                recipient_id: Some(target.id.to_string()),
            }
        }
    }
}

/// build_action_data 的物品预解析结果（item_id 与 source_type/quantity 联动）
struct PickedItem {
    item_id: String,
    /// "inventory" | "ground" | "resource"
    origin: &'static str,
    /// 已知存量（背包/地面），超量扣减在服务端必败
    available_qty: u32,
}

/// build_action_data 的接收方预解析结果（recipient_type 与 recipient_id 联动）
struct PickedRecipient {
    /// "agent" | "ground"
    recipient_type: &'static str,
    /// agent 分支的目标角色 uuid（ground 分支为 None）
    recipient_id: Option<String>,
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
        attrs.insert("satiation".into(), 50);
        attrs.insert("hydration".into(), 50);

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
                parent_chain: Vec::new(),
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
                survival_drives: vec![],
                status_effects: vec![],
                inventory: inv,
                skills: vec![],
                age_years: None,
                max_age: None,
                recipe_details: vec![],
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

    #[test]
    fn test_build_action_data_inventory_source_resolves_from_inventory() {
        // 消耗类（用）必须从背包解析：mock 世界背包有 test_item、地面有 ground_item，
        // 解析结果只能是背包物品（历史 bug：恒取地面物品导致服务端必败回滚）
        let ws = mock_world_state(80);
        let mut rng = rand::rng();
        let fields = vec!["item_id".to_string()];
        let data =
            ChaosGenerator::build_action_data(ItemActionSource::Inventory, &fields, &ws, &mut rng)
                .expect("背包非空应解析成功");
        assert_eq!(
            data.get("item_id").and_then(|v| v.as_str()),
            Some("test_item")
        );
    }

    #[test]
    fn test_build_action_data_skips_consume_when_inventory_empty() {
        // 空背包时消耗类动作不可解析（返回 None → 该动作被跳过，不产出必败意图）
        let mut ws = mock_world_state(80);
        ws.self_state.inventory = vec![];
        let mut rng = rand::rng();
        let fields = vec!["item_id".to_string()];
        assert!(
            ChaosGenerator::build_action_data(ItemActionSource::Inventory, &fields, &ws, &mut rng)
                .is_none()
        );
    }

    #[test]
    fn test_build_action_data_world_action_resolves_source_type_and_caps_quantity() {
        // 拾取类（取）：source_type 跟随物品实际来源（地面→ground），
        // quantity 以地面存量封顶（mock 存量 2，不得超量）
        let mut ws = mock_world_state(80);
        ws.nearby_items[0].quantity = 2;
        let mut rng = rand::rng();
        let fields = vec![
            "source_type".to_string(),
            "item_id".to_string(),
            "quantity".to_string(),
        ];
        let data =
            ChaosGenerator::build_action_data(ItemActionSource::World, &fields, &ws, &mut rng)
                .expect("地面物品在场应解析成功");
        assert_eq!(
            data.get("source_type").and_then(|v| v.as_str()),
            Some("ground")
        );
        assert_eq!(
            data.get("item_id").and_then(|v| v.as_str()),
            Some("ground_item")
        );
        let qty = data.get("quantity").and_then(|v| v.as_i64()).unwrap_or(99);
        assert!((1..=2).contains(&qty), "quantity 应以存量 2 封顶: {}", qty);
    }

    #[test]
    fn test_build_action_data_give_action_resolves_recipient_and_inventory_item() {
        // 予 的 chaos 行为：接收方随机二选一（附近任意角色 / 丢在地上），
        // 物品必须来自背包（Inventory 分类），agent 分支必须携带 recipient_id
        let ws = mock_world_state(80);
        let entity_ids: Vec<String> = ws.entities.iter().map(|e| e.id.to_string()).collect();
        let fields = vec![
            "recipient_type".to_string(),
            "item_id".to_string(),
            "quantity".to_string(),
        ];
        let mut saw_agent = false;
        let mut saw_ground = false;
        for _ in 0..40 {
            let mut rng = rand::rng();
            let data = ChaosGenerator::build_action_data(
                ItemActionSource::Inventory,
                &fields,
                &ws,
                &mut rng,
            )
            .expect("予 在背包非空时应可解析");
            let rtype = data
                .get("recipient_type")
                .and_then(|v| v.as_str())
                .expect("recipient_type 必须解析");
            match rtype {
                "agent" => {
                    saw_agent = true;
                    let rid = data
                        .get("recipient_id")
                        .and_then(|v| v.as_str())
                        .expect("agent 分支必须携带 recipient_id");
                    assert!(
                        entity_ids.contains(&rid.to_string()),
                        "recipient_id 必须来自附近实体: {}",
                        rid
                    );
                }
                "ground" => {
                    saw_ground = true;
                    assert!(
                        data.get("recipient_id").is_none(),
                        "ground 分支不应携带 recipient_id"
                    );
                }
                other => panic!("非法 recipient_type: {}", other),
            }
            assert_eq!(
                data.get("item_id").and_then(|v| v.as_str()),
                Some("test_item"),
                "予 的物品必须来自背包"
            );
        }
        // 40 次采样两分支均应出现（全偏一侧概率 2*(0.5^40)，可忽略）
        assert!(
            saw_agent && saw_ground,
            "chaos 行为应同时覆盖 agent 与 ground 两分支"
        );
    }

    #[test]
    fn test_build_action_data_give_action_falls_to_ground_when_no_entities() {
        // 附近无人：只能丢在地上，予 不因无人而整体跳过
        let mut ws = mock_world_state(80);
        ws.entities = vec![];
        let mut rng = rand::rng();
        let fields = vec![
            "recipient_type".to_string(),
            "item_id".to_string(),
            "quantity".to_string(),
        ];
        let data =
            ChaosGenerator::build_action_data(ItemActionSource::Inventory, &fields, &ws, &mut rng)
                .expect("无实体时应退化为 ground 分支");
        assert_eq!(
            data.get("recipient_type").and_then(|v| v.as_str()),
            Some("ground")
        );
        assert!(data.get("recipient_id").is_none());
    }
}
