// ============================================================================
// query_world ↔ compact_tool_result 流水线合同测试
// ============================================================================
//
// 目的：捕获 execute_query_world 输出 JSON 形状 与 compact_query_world
// 读取的 JSON 键 之间的漂移。漂移一旦发生，compactor 会静默失效（按
// 不存在的 key 取 None → 不做截断/清理 → LLM context window 被撑爆）。
//
// 测试策略：端到端地喂入 N 个实体/事件/物品（超过预算阈值），断言
// compactor 实际执行了截断与字段清理。任何一端改了 JSON 键，断言
// 失败，CI 立即发现漂移。
//
// DSH 对应抽象：dsh-tool-* 的 typed-args/typed-output 协议 —— Rust 侧以
// 合同测试代替 schemars 派生，保持零新依赖与最小侵入。
// ============================================================================

use std::collections::HashMap;

use cyber_jianghu_agent::component::state_store::WorldStateStore;
use cyber_jianghu_agent::models::{
    AgentSelfState, Entity, InventoryItem, Location, WorldEvent, WorldEventType, WorldState,
    WorldTime,
};
use cyber_jianghu_agent::soul::earth::compactor::compact_tool_result;
use cyber_jianghu_agent::soul::earth::state_tool::execute_query_world;

// 8K 模型典型预算（per_tool_limit ≈ 960 chars）
const SMALL_BUDGET: usize = 960;

// ---- 构造测试 WorldState ----

fn make_world_with_n_entities(n: usize) -> WorldState {
    WorldState {
        event_type: "world_state".to_string(),
        tick_id: 1,
        agent_id: None,
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
            attributes: HashMap::new(),
            derived_attributes: HashMap::new(),
            attribute_descriptions: HashMap::new(),
            survival_drives: vec![],
            status_effects: vec![],
            inventory: vec![],
            skills: vec![],
            recipe_details: vec![],
            age_years: None,
            max_age: None,
        },
        entities: (0..n)
            .map(|i| Entity {
                id: uuid::Uuid::new_v4(),
                name: format!("路人{}", i),
                distance: 0,
                state: "这是一段很长的状态描述，应该被 compactor 移除".to_string(),
                hostile: false,
                recent_actions: vec![],
            })
            .collect(),
        nearby_items: vec![],
        events_log: (0..n)
            .map(|i| WorldEvent {
                event_type: WorldEventType::ActionResult,
                tick_id: i as i64,
                description: format!("事件描述 {}", i),
                metadata: serde_json::json!({}),
            })
            .collect(),
        private_dialogue_log: vec![],
        last_execution_summary: None,
        lessons_learned: vec![],
    }
}

fn make_world_with_n_inventory(n: usize) -> WorldState {
    WorldState {
        event_type: "world_state".to_string(),
        tick_id: 1,
        agent_id: None,
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
            attributes: HashMap::new(),
            derived_attributes: HashMap::new(),
            attribute_descriptions: HashMap::new(),
            survival_drives: vec![],
            status_effects: vec![],
            inventory: (0..n)
                .map(|i| InventoryItem {
                    item_id: format!("item-{}", i),
                    name: format!("物品{}", i),
                    quantity: 1,
                    is_equipped: false,
                    item_type: "consumable".to_string(),
                })
                .collect(),
            skills: vec![],
            recipe_details: vec![],
            age_years: None,
            max_age: None,
        },
        entities: vec![],
        nearby_items: vec![],
        events_log: vec![],
        private_dialogue_log: vec![],
        last_execution_summary: None,
        lessons_learned: vec![],
    }
}

// ============================================================================
// 合同测试：entities 流水线
// ============================================================================
//
// 约束（来自 compactor.rs AVG_ENTITY_CHARS=60 + clamp(3,30)）：
//   960 / 60 = 16 → entities 应被截断到 ≤ 16
//   同时 entity.state 字段应被移除
//
// 若 execute_query_world 把 "entities" 改名 "npcs"，compactor 找不到 key，
// 不做截断/清理 → 下面断言失败，捕获漂移。
#[tokio::test]
async fn contract_query_world_entities_pipeline_truncates_and_strips_state() {
    let store = WorldStateStore::new();
    store.update(make_world_with_n_entities(20)).await;

    let raw = execute_query_world("entities", None, &store).await;

    // 锁定上游契约：execute_query_world 输出必须有 "entities" 键且为数组
    assert_eq!(raw["section"], "entities");
    assert_eq!(raw["entities"].as_array().unwrap().len(), 20);

    let compacted = compact_tool_result("query_world", &raw, SMALL_BUDGET);

    // 锁定下游契约：compactor 必须读得到 "entities" 键
    let entities = compacted["entities"]
        .as_array()
        .expect("合同违反：compactor 应读取 \"entities\" 键，但未找到");
    assert!(
        entities.len() <= 16,
        "合同违反：compactor 应将 entities 截断到 ≤16（实际 {}），\n\
         可能原因：execute_query_world 输出键名漂移，或 AVG_ENTITY_CHARS 不匹配",
        entities.len()
    );

    // 锁定字段清理契约：每条 entity 的 state 字段必须被剥离
    for (i, e) in entities.iter().enumerate() {
        assert!(
            e.get("state").is_none(),
            "合同违反：entity[{}].state 应被 compactor 移除",
            i
        );
    }
}

// ============================================================================
// 合同测试：events 流水线
// ============================================================================
//
// 约束（AVG_EVENT_CHARS=80 + clamp(3,20)）：960 / 80 = 12
#[tokio::test]
async fn contract_query_world_events_pipeline_truncates() {
    let store = WorldStateStore::new();
    store.update(make_world_with_n_entities(20)).await;

    let raw = execute_query_world("events", None, &store).await;
    assert_eq!(raw["events"].as_array().unwrap().len(), 20);

    let compacted = compact_tool_result("query_world", &raw, SMALL_BUDGET);

    let events = compacted["events"]
        .as_array()
        .expect("合同违反：compactor 应读取 \"events\" 键");
    assert!(
        events.len() <= 12,
        "合同违反：events 应被截断到 ≤12（实际 {}），\n\
         可能原因：execute_query_world 输出键名漂移，或 AVG_EVENT_CHARS 不匹配",
        events.len()
    );
}

// ============================================================================
// 合同测试：inventory 流水线
// ============================================================================
//
// 约束（AVG_ITEM_CHARS=60 + clamp(5,50)）：960 / 60 = 16，clamp 到 5..50 → 16
#[tokio::test]
async fn contract_query_world_inventory_pipeline_truncates() {
    let store = WorldStateStore::new();
    store.update(make_world_with_n_inventory(60)).await;

    let raw = execute_query_world("inventory", None, &store).await;
    assert_eq!(raw["total"], 60);
    assert_eq!(raw["items"].as_array().unwrap().len(), 60);

    let compacted = compact_tool_result("query_world", &raw, SMALL_BUDGET);

    let items = compacted["items"]
        .as_array()
        .expect("合同违反：compactor 应读取 \"items\" 键");
    assert!(
        items.len() <= 50,
        "合同违反：items 应被截断到 ≤50（实际 {}），\n\
         可能原因：execute_query_world 输出键名漂移，或 AVG_ITEM_CHARS 不匹配",
        items.len()
    );
    assert!(
        items.len() >= 5,
        "合同违反：items 下限 clamp(5,50) 不应被绕过（实际 {}）",
        items.len()
    );
}

// ============================================================================
// 合同测试：environment / state 是 no-op 流水线
// ============================================================================
//
// 约束：这两类 section 没有数组，compact_query_world 对它们什么都不做。
// 若 execute_query_world 输出意外地包含 "entities"/"items" 字段，
// compactor 会按 section == "environment" 匹配到 no-op 分支 → 不会损坏。
// 此测试断言 compactor 对 environment/state 不破坏结构。
#[tokio::test]
async fn contract_query_world_environment_state_pipeline_is_noop() {
    let store = WorldStateStore::new();
    store.update(make_world_with_n_entities(5)).await;

    for section in ["environment", "state"] {
        let raw = execute_query_world(section, None, &store).await;
        let compacted = compact_tool_result("query_world", &raw, SMALL_BUDGET);
        // no-op 应当原样返回 success / section 字段
        assert_eq!(
            compacted["success"], true,
            "section={} 应保持 success=true",
            section
        );
        assert_eq!(compacted["section"], section);
    }
}
