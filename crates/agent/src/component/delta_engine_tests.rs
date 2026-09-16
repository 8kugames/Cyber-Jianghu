//! delta_engine 模块单测（自 delta_engine.rs 外移，内容未改）

use super::*;
use cyber_jianghu_protocol::{
    AgentSelfState, Entity, InventoryItem, Location, WorldEvent, WorldEventType, WorldTime,
};
use uuid::Uuid;

fn test_config() -> DeltaConfig {
    DeltaConfig {
        change_percentage_threshold: 0.1,
        survival_critical_urgency_threshold: 5,
        attribute_display_names: Default::default(),
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
    }
}

fn default_location() -> Location {
    Location {
        node_id: "loc_01".to_string(),
        name: "客栈".to_string(),
        node_type: "inn".to_string(),
        adjacent_nodes: vec![],
        gatherable_items: vec![],
        parent_chain: Vec::new(),
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
        parent_chain: Vec::new(),
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
