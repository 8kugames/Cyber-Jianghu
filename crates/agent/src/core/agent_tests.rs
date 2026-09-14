// agent.rs 单元测试（经 #[path] 挂载为 crate::core::agent::tests，测试路径保持不变）
use super::*;
use futures_util::future::BoxFuture;

use cyber_jianghu_protocol::{AgentSelfState, Location, WorldTime};

fn test_config() -> Config {
    Config {
        server: crate::config::ServerConfig::default(),
        runtime: crate::config::RuntimeConfig::default(),
        llm: crate::config::LlmConfig::default(),
        llm_reflector: None,
        memory: crate::config::MemoryConfig::default(),
        game_rules: None,
        config_path: PathBuf::from("/tmp/test-agent-config.yaml"),
        servers_dir: PathBuf::from("/tmp/test-agent-servers"),
        earth_soul: crate::soul::earth::config::EarthSoulConfig::default(),
        token_optimization: crate::config::TokenOptimizationConfig::default(),
        character_generation: crate::config::CharacterGenerationConfig {
            world_setting: "测试世界".to_string(),
            fields: Vec::new(),
        },
    }
}

fn noop_decision_callback() -> crate::runtime::DecisionCallback {
    Arc::new(
        |tick_id: i64, agent_id: Uuid| -> BoxFuture<'static, Intent> {
            Box::pin(async move { Intent::new(agent_id, tick_id, "休整", None) })
        },
    )
}

/// 构造最小可用 WorldState（仅填 required 字段，用于 build_tick_memory_context 集成测试）
fn test_world_state(tick_id: i64) -> cyber_jianghu_protocol::WorldState {
    cyber_jianghu_protocol::WorldState {
        event_type: "world_state".to_string(),
        tick_id,
        agent_id: Some(Uuid::new_v4()),
        world_time: WorldTime {
            year: 1,
            month: 1,
            day: 1,
            hour: 8,
            minute: 0,
            second: 0,
            weather: "晴".to_string(),
        },
        location: Location {
            node_id: "loc_a".to_string(),
            name: "地点A".to_string(),
            node_type: "inn".to_string(),
            adjacent_nodes: vec![],
            gatherable_items: vec![],
            parent_chain: Vec::new(),
        },
        self_state: AgentSelfState {
            attributes: std::collections::HashMap::new(),
            derived_attributes: std::collections::HashMap::new(),
            attribute_descriptions: std::collections::HashMap::new(),
            survival_drives: vec![],
            status_effects: vec![],
            inventory: vec![],
            skills: vec![],
            age_years: None,
            max_age: None,
            recipe_details: vec![],
        },
        entities: vec![],
        nearby_items: vec![],
        events_log: vec![],
        private_dialogue_log: vec![],
        last_execution_summary: None,
    }
}

#[test]
fn test_rejection_feedback_expired_by_ttl() {
    let set_at = 100_i64;
    // 产生 tick 当轮及紧邻下一 tick 内仍可见（不超 TTL）
    assert!(!Agent::rejection_feedback_expired(set_at, set_at));
    assert!(!Agent::rejection_feedback_expired(
        set_at,
        set_at + crate::config::REJECTION_FEEDBACK_TTL_TICKS
    ));
    // 超过 TTL 一个 tick 即过期
    assert!(Agent::rejection_feedback_expired(
        set_at,
        set_at + crate::config::REJECTION_FEEDBACK_TTL_TICKS + 1
    ));
}

#[tokio::test]
async fn test_set_rejection_feedback_records_tick() {
    let mut agent = Agent::new(test_config(), noop_decision_callback(), None, None).await;

    agent.set_rejection_feedback("目标角色 x 不在附近实体中", 42);

    assert!(agent.last_rejection_reason.is_some());
    assert_eq!(agent.last_rejection_tick, Some(42));
}

#[tokio::test]
async fn test_build_tick_context_expires_stale_rejection_feedback() {
    let mut agent = Agent::new(test_config(), noop_decision_callback(), None, None).await;
    // tick 98 设置，当前 tick 100：差值 2 > TTL=1 → 过期清除
    agent.set_rejection_feedback("旧驳回", 98);
    let world_state = test_world_state(100);

    agent.build_tick_memory_context(&world_state).await;

    assert!(agent.last_rejection_reason.is_none());
    assert!(agent.last_rejection_tick.is_none());
}

#[tokio::test]
async fn test_build_tick_context_keeps_fresh_rejection_feedback() {
    let mut agent = Agent::new(test_config(), noop_decision_callback(), None, None).await;
    // tick 100 设置，当前 tick 101：差值 1 ≤ TTL=1 → 保留供本 tick 决策参考
    agent.set_rejection_feedback("新驳回", 100);
    let world_state = test_world_state(101);

    agent.build_tick_memory_context(&world_state).await;

    assert!(agent.last_rejection_reason.is_some());
    assert_eq!(agent.last_rejection_tick, Some(100));
}

#[tokio::test]
async fn test_reload_character_persona_updates_persona_name_without_engine() {
    let repo_config_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("agent crate parent")
        .join("server/config");
    unsafe {
        std::env::set_var("CYBER_JIANGHU_CONFIG_DIR", &repo_config_dir);
    }

    let mut agent = Agent::new(test_config(), noop_decision_callback(), None, None).await;

    assert_eq!(agent.persona.read(|p| p.name.clone()), "无名侠客");

    agent.reload_character_persona(Uuid::new_v4(), "裴无咎");

    assert_eq!(agent.persona.read(|p| p.name.clone()), "裴无咎");
}
