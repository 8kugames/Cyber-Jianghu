// ============================================================================
// Delta Engine — WorldState 变化检测
// 纯规则引擎，零 LLM token 消耗
// ============================================================================

use cyber_jianghu_protocol::WorldState;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// 变化类别
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChangeCategory {
    Survival,
    Social,
    Environment,
    Inventory,
    Location,
}

/// 紧急程度
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Urgency {
    Critical,
    Important,
    Info,
}

/// 检测到的状态变化
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateChange {
    pub category: ChangeCategory,
    pub urgency: Urgency,
    pub field: String,
    pub description: String,
    pub data: serde_json::Value,
    pub tool_hint: Option<String>,
}

/// 变化检测结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateDelta {
    pub changes: Vec<StateChange>,
    pub is_first_tick: bool,
}

/// Delta 检测配置
#[derive(Debug, Clone)]
pub struct DeltaConfig {
    /// 变化百分比阈值（|diff| / 100 >= threshold => Important）
    pub change_percentage_threshold: f32,
    /// 生存驱动 Critical 阈值：只有 survival_drive.urgency >= 此值时才标 Critical
    /// 默认值 5 对应 narratives.yaml 中 satiation/hydration urgency=3(轻微), 7(重度), 10(致命)
    /// 低于此值的生存属性变化标 Important 而非 Critical，减少信号噪声
    pub survival_critical_urgency_threshold: u8,
}

impl Default for DeltaConfig {
    fn default() -> Self {
        Self {
            change_percentage_threshold: 0.1,
            survival_critical_urgency_threshold: 5,
        }
    }
}

/// Delta Engine: 比较 prev vs curr WorldState
pub struct DeltaEngine {
    config: DeltaConfig,
}

impl DeltaEngine {
    pub fn new(config: DeltaConfig) -> Self {
        Self { config }
    }

    /// 计算 prev → curr 的变化量
    pub fn compute(&self, prev: Option<&WorldState>, curr: &WorldState) -> StateDelta {
        let is_first_tick = prev.is_none();
        let mut changes = Vec::new();

        match prev {
            None => {
                self.detect_full_state(curr, &mut changes);
            }
            Some(prev) => {
                self.detect_survival_changes(
                    &curr.self_state.attributes,
                    &prev.self_state.attributes,
                    &curr.self_state.survival_drives,
                    &mut changes,
                );
                self.detect_social_changes(&curr.entities, &prev.entities, &mut changes);
                self.detect_environment_changes(&curr.events_log, &prev.events_log, &mut changes);
                self.detect_inventory_changes(
                    &curr.self_state.inventory,
                    &prev.self_state.inventory,
                    &mut changes,
                );
                self.detect_location_changes(&curr.location, &prev.location, &mut changes);
            }
        }

        StateDelta {
            changes,
            is_first_tick,
        }
    }

    /// 首次 tick：生成全量状态快照
    fn detect_full_state(&self, curr: &WorldState, changes: &mut Vec<StateChange>) {
        let survival_urgencies: HashMap<&str, u8> = curr
            .self_state
            .survival_drives
            .iter()
            .map(|sd| (sd.attribute.as_str(), sd.urgency))
            .collect();

        // 属性
        for (key, &val) in &curr.self_state.attributes {
            let urgency = match survival_urgencies.get(key.as_str()) {
                Some(&drive_urgency)
                    if drive_urgency >= self.config.survival_critical_urgency_threshold =>
                {
                    Urgency::Critical
                }
                Some(_) => Urgency::Important,
                None => Urgency::Important,
            };
            changes.push(StateChange {
                category: ChangeCategory::Survival,
                urgency,
                field: format!("attributes.{}", key),
                description: format!("初始状态 {}: {}", key, val),
                data: serde_json::json!({ key: val }),
                tool_hint: None,
            });
        }

        // 实体
        for entity in &curr.entities {
            changes.push(StateChange {
                category: ChangeCategory::Social,
                urgency: Urgency::Important,
                field: "entities".to_string(),
                description: format!("附近存在: {}", entity.name),
                data: serde_json::json!({ "id": entity.id, "name": entity.name }),
                tool_hint: Some(format!(
                    "query_world(section=entities, filter={})",
                    entity.name
                )),
            });
        }

        // 位置
        changes.push(StateChange {
            category: ChangeCategory::Location,
            urgency: Urgency::Important,
            field: "location".to_string(),
            description: format!(
                "当前位置: {} ({})",
                curr.location.name, curr.location.node_id
            ),
            data: serde_json::json!({
                "node_id": curr.location.node_id,
                "name": curr.location.name,
            }),
            tool_hint: Some("query_world(section=environment)".to_string()),
        });

        // 事件
        for event in &curr.events_log {
            changes.push(StateChange {
                category: ChangeCategory::Environment,
                urgency: Urgency::Important,
                field: "events_log".to_string(),
                description: format!("事件: {}", event.description),
                data: serde_json::to_value(event).unwrap_or_default(),
                tool_hint: Some("query_world(section=events)".to_string()),
            });
        }

        // 背包
        if !curr.self_state.inventory.is_empty() {
            changes.push(StateChange {
                category: ChangeCategory::Inventory,
                urgency: Urgency::Important,
                field: "inventory".to_string(),
                description: format!("背包有 {} 件物品", curr.self_state.inventory.len()),
                data: serde_json::json!(curr.self_state.inventory.len()),
                tool_hint: Some("query_world(section=inventory)".to_string()),
            });
        }
    }

    /// 检测属性变化（数据驱动：从 server 下发的 survival_drives 判定 Critical）
    ///
    /// Critical 判定规则：survival_drive.urgency >= config.survival_critical_urgency_threshold
    /// 而不是简单地检查属性是否在 survival_drives 中，以避免低紧迫度的生存属性变化
    /// （如 hydration=59→58, urgency=3）产生 Critical 信号噪声。
    fn detect_survival_changes(
        &self,
        curr_attrs: &HashMap<String, i32>,
        prev_attrs: &HashMap<String, i32>,
        survival_drives: &[cyber_jianghu_protocol::SurvivalDrive],
        changes: &mut Vec<StateChange>,
    ) {
        // 构建 attribute -> urgency 映射（仅 urgency > 0 的驱动）
        let survival_urgencies: HashMap<&str, u8> = survival_drives
            .iter()
            .map(|sd| (sd.attribute.as_str(), sd.urgency))
            .collect();

        for (key, &curr_val) in curr_attrs {
            let prev_val = prev_attrs.get(key).copied().unwrap_or(0);
            if curr_val == prev_val {
                continue;
            }
            let diff = (curr_val - prev_val).unsigned_abs();
            let urgency = match survival_urgencies.get(key.as_str()) {
                Some(&drive_urgency)
                    if drive_urgency >= self.config.survival_critical_urgency_threshold =>
                {
                    Urgency::Critical
                }
                Some(_) => Urgency::Important, // 生存属性但紧迫不足 → Important
                None if diff as f32 / 100.0 >= self.config.change_percentage_threshold => {
                    Urgency::Important
                }
                _ => Urgency::Info,
            };

            changes.push(StateChange {
                category: ChangeCategory::Survival,
                urgency,
                field: format!("attributes.{}", key),
                description: format!("{}: {} -> {}", key, prev_val, curr_val),
                data: serde_json::json!({ "key": key, "prev": prev_val, "curr": curr_val }),
                tool_hint: Some("query_world(section=state)".to_string()),
            });
        }
    }

    /// 检测实体变化（出现/消失）
    fn detect_social_changes(
        &self,
        curr_entities: &[cyber_jianghu_protocol::Entity],
        prev_entities: &[cyber_jianghu_protocol::Entity],
        changes: &mut Vec<StateChange>,
    ) {
        let curr_ids: HashSet<uuid::Uuid> = curr_entities.iter().map(|e| e.id).collect();
        let prev_ids: HashSet<uuid::Uuid> = prev_entities.iter().map(|e| e.id).collect();

        // 新出现
        for entity in curr_entities {
            if !prev_ids.contains(&entity.id) {
                changes.push(StateChange {
                    category: ChangeCategory::Social,
                    urgency: Urgency::Important,
                    field: "entities".to_string(),
                    description: format!("{} 出现", entity.name),
                    data: serde_json::json!({ "id": entity.id, "name": entity.name }),
                    tool_hint: Some(format!(
                        "query_world(section=entities, filter={})",
                        entity.name
                    )),
                });
            }
        }

        // 离开
        for entity in prev_entities {
            if !curr_ids.contains(&entity.id) {
                changes.push(StateChange {
                    category: ChangeCategory::Social,
                    urgency: Urgency::Info,
                    field: "entities".to_string(),
                    description: format!("{} 离开", entity.name),
                    data: serde_json::json!({ "id": entity.id, "name": entity.name }),
                    tool_hint: None,
                });
            }
        }
    }

    /// 检测新事件（events_log 末尾追加）
    fn detect_environment_changes(
        &self,
        curr_events: &[cyber_jianghu_protocol::WorldEvent],
        prev_events: &[cyber_jianghu_protocol::WorldEvent],
        changes: &mut Vec<StateChange>,
    ) {
        if curr_events.len() > prev_events.len() {
            for event in &curr_events[prev_events.len()..] {
                changes.push(StateChange {
                    category: ChangeCategory::Environment,
                    urgency: Urgency::Important,
                    field: "events_log".to_string(),
                    description: event.description.clone(),
                    data: serde_json::to_value(event).unwrap_or_default(),
                    tool_hint: Some("query_world(section=events)".to_string()),
                });
            }
        }
    }

    /// 检测背包变化
    fn detect_inventory_changes(
        &self,
        curr_inv: &[cyber_jianghu_protocol::InventoryItem],
        prev_inv: &[cyber_jianghu_protocol::InventoryItem],
        changes: &mut Vec<StateChange>,
    ) {
        let curr_map: HashMap<&str, &cyber_jianghu_protocol::InventoryItem> =
            curr_inv.iter().map(|i| (i.item_id.as_str(), i)).collect();
        let prev_map: HashMap<&str, &cyber_jianghu_protocol::InventoryItem> =
            prev_inv.iter().map(|i| (i.item_id.as_str(), i)).collect();

        // 新增或数量变化
        for (id, item) in &curr_map {
            match prev_map.get(id) {
                None => {
                    changes.push(StateChange {
                        category: ChangeCategory::Inventory,
                        urgency: Urgency::Info,
                        field: format!("inventory.{}", id),
                        description: format!("获得 {} x{}", item.name, item.quantity),
                        data: serde_json::json!({ "item_id": id, "quantity": item.quantity }),
                        tool_hint: Some("query_world(section=inventory)".to_string()),
                    });
                }
                Some(prev_item) if prev_item.quantity != item.quantity => {
                    let urgency = if item.quantity < prev_item.quantity {
                        Urgency::Important
                    } else {
                        Urgency::Info
                    };
                    changes.push(StateChange {
                        category: ChangeCategory::Inventory,
                        urgency,
                        field: format!("inventory.{}", id),
                        description: format!(
                            "{}: {} -> {}",
                            item.name, prev_item.quantity, item.quantity
                        ),
                        data: serde_json::json!({
                            "item_id": id,
                            "prev": prev_item.quantity,
                            "curr": item.quantity,
                        }),
                        tool_hint: Some("query_world(section=inventory)".to_string()),
                    });
                }
                _ => {}
            }
        }

        // 移除
        for (id, item) in &prev_map {
            if !curr_map.contains_key(id) {
                changes.push(StateChange {
                    category: ChangeCategory::Inventory,
                    urgency: Urgency::Important,
                    field: format!("inventory.{}", id),
                    description: format!("失去 {}", item.name),
                    data: serde_json::json!({ "item_id": id, "lost": true }),
                    tool_hint: None,
                });
            }
        }
    }

    /// 检测位置变化
    fn detect_location_changes(
        &self,
        curr_loc: &cyber_jianghu_protocol::Location,
        prev_loc: &cyber_jianghu_protocol::Location,
        changes: &mut Vec<StateChange>,
    ) {
        if curr_loc.node_id != prev_loc.node_id {
            changes.push(StateChange {
                category: ChangeCategory::Location,
                urgency: Urgency::Important,
                field: "location.node_id".to_string(),
                description: format!("移动: {} -> {}", prev_loc.name, curr_loc.name),
                data: serde_json::json!({
                    "prev": { "node_id": prev_loc.node_id, "name": prev_loc.name },
                    "curr": { "node_id": curr_loc.node_id, "name": curr_loc.name },
                }),
                tool_hint: Some("query_world(section=environment)".to_string()),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_jianghu_protocol::{
        AgentSelfState, Entity, InventoryItem, Location, WorldEvent, WorldEventType, WorldTime,
    };
    use uuid::Uuid;

    fn test_config() -> DeltaConfig {
        DeltaConfig {
            change_percentage_threshold: 0.1,
            survival_critical_urgency_threshold: 5,
        }
    }

    fn test_engine() -> DeltaEngine {
        DeltaEngine::new(test_config())
    }

    fn default_world_time() -> WorldTime {
        WorldTime {
            year: 1,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            weather: "晴".to_string(),
        }
    }

    /// 构建最小 WorldState
    fn build_world_state(
        attrs: HashMap<String, i32>,
        entities: Vec<Entity>,
        events: Vec<WorldEvent>,
        inventory: Vec<InventoryItem>,
        location: Location,
    ) -> WorldState {
        build_world_state_with_drives(attrs, vec![], entities, events, inventory, location)
    }

    fn build_world_state_with_drives(
        attrs: HashMap<String, i32>,
        survival_drives: Vec<cyber_jianghu_protocol::SurvivalDrive>,
        entities: Vec<Entity>,
        events: Vec<WorldEvent>,
        inventory: Vec<InventoryItem>,
        location: Location,
    ) -> WorldState {
        WorldState {
            event_type: "world_state".to_string(),
            tick_id: 1,
            agent_id: Some(Uuid::new_v4()),
            world_time: default_world_time(),
            location,
            self_state: AgentSelfState {
                attributes: attrs,
                derived_attributes: HashMap::new(),
                attribute_descriptions: HashMap::new(),
                survival_drives,
                status_effects: vec![],
                inventory,
                skills: vec![],
                recipe_details: vec![],
                age_years: None,
                max_age: None,
            },
            entities,
            events_log: events,
            nearby_items: vec![],
            private_dialogue_log: vec![],
            last_execution_summary: None,
            lessons_learned: vec![],
        }
    }

    fn default_location() -> Location {
        Location {
            node_id: "loc_01".to_string(),
            name: "客栈".to_string(),
            node_type: "inn".to_string(),
            adjacent_nodes: vec![],
            gatherable_items: vec![],
        }
    }

    fn make_event(desc: &str) -> WorldEvent {
        WorldEvent {
            event_type: WorldEventType::ActionResult,
            tick_id: 1,
            description: desc.to_string(),
            metadata: serde_json::json!({}),
        }
    }

    fn make_entity(name: &str) -> Entity {
        Entity {
            id: Uuid::new_v4(),
            name: name.to_string(),
            distance: 0,
            state: "alive".to_string(),
            hostile: false,
            recent_actions: vec![],
        }
    }

    fn make_item(id: &str, name: &str, qty: i32) -> InventoryItem {
        InventoryItem {
            item_id: id.to_string(),
            name: name.to_string(),
            quantity: qty,
            is_equipped: false,
            item_type: "consumable".to_string(),
        }
    }

    #[test]
    fn test_first_tick_generates_full_state() {
        let engine = test_engine();
        let mut attrs = HashMap::new();
        attrs.insert("hp".to_string(), 80);
        attrs.insert("satiation".to_string(), 30);
        let entity = make_entity("张三");
        let loc = default_location();
        let event = make_event("有人打架");

        let ws = build_world_state(
            attrs,
            vec![entity],
            vec![event],
            vec![make_item("bread", "面包", 2)],
            loc,
        );

        let delta = engine.compute(None, &ws);
        assert!(delta.is_first_tick);
        assert!(!delta.changes.is_empty());

        // 应包含属性、实体、位置、事件、背包变化
        let categories: HashSet<_> = delta.changes.iter().map(|c| c.category.clone()).collect();
        assert!(categories.contains(&ChangeCategory::Survival));
        assert!(categories.contains(&ChangeCategory::Social));
        assert!(categories.contains(&ChangeCategory::Location));
        assert!(categories.contains(&ChangeCategory::Environment));
        assert!(categories.contains(&ChangeCategory::Inventory));

        // 首次 tick 全部为 Important 或 Critical
        for change in &delta.changes {
            assert!(
                change.urgency == Urgency::Important || change.urgency == Urgency::Critical,
                "首次 tick 变化应为 Important 或 Critical，实际: {:?}",
                change.urgency
            );
        }
    }

    #[test]
    fn test_survival_critical_threshold() {
        let engine = test_engine();
        let prev_attrs = HashMap::from([("hp".to_string(), 50), ("satiation".to_string(), 50)]);
        let curr_attrs = HashMap::from([("hp".to_string(), 20), ("satiation".to_string(), 50)]);

        let prev = build_world_state(prev_attrs, vec![], vec![], vec![], default_location());
        // server 预计算：hp=20 触发生存驱动 → Critical
        let drives = vec![cyber_jianghu_protocol::SurvivalDrive {
            attribute: "hp".to_string(),
            drive: "疗伤".to_string(),
            reason: "受伤".to_string(),
            urgency: 8,
            goal: "治疗".to_string(),
        }];
        let curr = build_world_state_with_drives(
            curr_attrs,
            drives,
            vec![],
            vec![],
            vec![],
            default_location(),
        );

        let delta = engine.compute(Some(&prev), &curr);
        let hp_change = delta
            .changes
            .iter()
            .find(|c| c.field == "attributes.hp")
            .expect("应有 hp 变化");
        assert_eq!(hp_change.urgency, Urgency::Critical);
    }

    #[test]
    fn test_survival_low_urgency_not_critical() {
        let engine = test_engine();
        let prev_attrs = HashMap::from([("satiation".to_string(), 42)]);
        let curr_attrs = HashMap::from([("satiation".to_string(), 30)]);

        let drives = vec![cyber_jianghu_protocol::SurvivalDrive {
            attribute: "satiation".to_string(),
            drive: "寻找食物".to_string(),
            reason: "肚子饿了".to_string(),
            urgency: 3,
            goal: "找东西吃".to_string(),
        }];

        let prev = build_world_state(prev_attrs, vec![], vec![], vec![], default_location());
        let curr = build_world_state_with_drives(
            curr_attrs,
            drives,
            vec![],
            vec![],
            vec![],
            default_location(),
        );

        let delta = engine.compute(Some(&prev), &curr);
        let satiation_change = delta
            .changes
            .iter()
            .find(|c| c.field == "attributes.satiation")
            .expect("应有 satiation 变化");
        assert_eq!(
            satiation_change.urgency,
            Urgency::Important,
            "urgency=3 应标 Important 而非 Critical"
        );
    }

    #[test]
    fn test_survival_important_change() {
        let engine = test_engine();
        // change_percentage_threshold = 0.1 → 变化 >= 10 时 Important（但未超阈值）
        let prev_attrs = HashMap::from([("hp".to_string(), 80)]);
        let curr_attrs = HashMap::from([("hp".to_string(), 65)]);

        let prev = build_world_state(prev_attrs, vec![], vec![], vec![], default_location());
        let mut curr = prev.clone();
        curr.self_state.attributes = curr_attrs;

        let delta = engine.compute(Some(&prev), &curr);
        let hp_change = delta
            .changes
            .iter()
            .find(|c| c.field == "attributes.hp")
            .expect("应有 hp 变化");
        assert_eq!(hp_change.urgency, Urgency::Important);
    }

    #[test]
    fn test_survival_no_change() {
        let engine = test_engine();
        let attrs = HashMap::from([("hp".to_string(), 80), ("satiation".to_string(), 50)]);

        let prev = build_world_state(attrs.clone(), vec![], vec![], vec![], default_location());
        let curr = build_world_state(attrs, vec![], vec![], vec![], default_location());

        let delta = engine.compute(Some(&prev), &curr);
        let survival_changes: Vec<_> = delta
            .changes
            .iter()
            .filter(|c| c.category == ChangeCategory::Survival)
            .collect();
        assert!(
            survival_changes.is_empty(),
            "属性未变化时不应产生 Survival 变化"
        );
    }

    #[test]
    fn test_social_new_entity() {
        let engine = test_engine();
        let entity = make_entity("李四");

        let prev = build_world_state(HashMap::new(), vec![], vec![], vec![], default_location());
        let curr = build_world_state(
            HashMap::new(),
            vec![entity],
            vec![],
            vec![],
            default_location(),
        );

        let delta = engine.compute(Some(&prev), &curr);
        let social: Vec<_> = delta
            .changes
            .iter()
            .filter(|c| c.category == ChangeCategory::Social)
            .collect();
        assert_eq!(social.len(), 1);
        assert_eq!(social[0].urgency, Urgency::Important);
        assert!(social[0].description.contains("李四"));
        assert!(social[0].description.contains("出现"));
    }

    #[test]
    fn test_social_entity_leaves() {
        let engine = test_engine();
        let entity = make_entity("王五");

        let prev = build_world_state(
            HashMap::new(),
            vec![entity],
            vec![],
            vec![],
            default_location(),
        );
        let curr = build_world_state(HashMap::new(), vec![], vec![], vec![], default_location());

        let delta = engine.compute(Some(&prev), &curr);
        let social: Vec<_> = delta
            .changes
            .iter()
            .filter(|c| c.category == ChangeCategory::Social)
            .collect();
        assert_eq!(social.len(), 1);
        assert_eq!(social[0].urgency, Urgency::Info);
        assert!(social[0].description.contains("王五"));
        assert!(social[0].description.contains("离开"));
    }

    #[test]
    fn test_environment_new_events() {
        let engine = test_engine();
        let e1 = make_event("有人打架");
        let e2 = make_event("天降大雨");

        let prev = build_world_state(
            HashMap::new(),
            vec![],
            vec![e1.clone()],
            vec![],
            default_location(),
        );
        let curr = build_world_state(
            HashMap::new(),
            vec![],
            vec![e1, e2],
            vec![],
            default_location(),
        );

        let delta = engine.compute(Some(&prev), &curr);
        let env_changes: Vec<_> = delta
            .changes
            .iter()
            .filter(|c| c.category == ChangeCategory::Environment)
            .collect();
        assert_eq!(env_changes.len(), 1);
        assert_eq!(env_changes[0].urgency, Urgency::Important);
        assert!(env_changes[0].description.contains("天降大雨"));
    }

    #[test]
    fn test_inventory_quantity_change() {
        let engine = test_engine();

        let prev = build_world_state(
            HashMap::new(),
            vec![],
            vec![],
            vec![make_item("bread", "面包", 5)],
            default_location(),
        );
        let curr = build_world_state(
            HashMap::new(),
            vec![],
            vec![],
            vec![make_item("bread", "面包", 3)],
            default_location(),
        );

        let delta = engine.compute(Some(&prev), &curr);
        let inv: Vec<_> = delta
            .changes
            .iter()
            .filter(|c| c.category == ChangeCategory::Inventory)
            .collect();
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].urgency, Urgency::Important); // 减少是 Important
        assert!(inv[0].description.contains("5 -> 3"));
    }

    #[test]
    fn test_inventory_item_lost() {
        let engine = test_engine();

        let prev = build_world_state(
            HashMap::new(),
            vec![],
            vec![],
            vec![make_item("sword", "铁剑", 1)],
            default_location(),
        );
        let curr = build_world_state(HashMap::new(), vec![], vec![], vec![], default_location());

        let delta = engine.compute(Some(&prev), &curr);
        let inv: Vec<_> = delta
            .changes
            .iter()
            .filter(|c| c.category == ChangeCategory::Inventory)
            .collect();
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].urgency, Urgency::Important);
        assert!(inv[0].description.contains("失去"));
        assert!(inv[0].description.contains("铁剑"));
    }

    #[test]
    fn test_location_change() {
        let engine = test_engine();
        let prev_loc = default_location();
        let curr_loc = Location {
            node_id: "loc_02".to_string(),
            name: "街道".to_string(),
            node_type: "street".to_string(),
            adjacent_nodes: vec![],
            gatherable_items: vec![],
        };

        let prev = build_world_state(HashMap::new(), vec![], vec![], vec![], prev_loc);
        let curr = build_world_state(HashMap::new(), vec![], vec![], vec![], curr_loc);

        let delta = engine.compute(Some(&prev), &curr);
        let loc_changes: Vec<_> = delta
            .changes
            .iter()
            .filter(|c| c.category == ChangeCategory::Location)
            .collect();
        assert_eq!(loc_changes.len(), 1);
        assert_eq!(loc_changes[0].urgency, Urgency::Important);
        assert!(loc_changes[0].description.contains("客栈"));
        assert!(loc_changes[0].description.contains("街道"));
    }
}
