use std::collections::HashMap;
use tempfile::TempDir;
use uuid::Uuid;

use cyber_jianghu_agent::component::persona::trait_types::{Trait, TraitChange, TraitType};
use cyber_jianghu_agent::component::persona::{
    DynamicPersona, PersonaPersistenceConfig, PersonaState, PersonaStore, ThreadSafePersona,
};

fn make_default_persona(agent_id: Uuid, name: &str) -> DynamicPersona {
    DynamicPersona::new(agent_id, name, "test description")
}

fn stress_state(level: u8) -> PersonaState {
    PersonaState {
        current_emotion: "焦虑".to_string(),
        current_goal: Some("寻找食物".to_string()),
        stress_level: level,
        last_updated: 100,
    }
}

#[test]
fn open_creates_db_and_persists_default_when_no_row() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("persona.db");
    let agent_id = Uuid::new_v4();

    let store = PersonaStore::open(agent_id, &db, PersonaPersistenceConfig::default()).unwrap();
    let default = make_default_persona(agent_id, "林远图");

    let loaded = store.load_or_default(default.clone()).unwrap();
    assert_eq!(loaded.name, "林远图");
    assert_eq!(loaded.base_description, "test description");
    assert!(!loaded.traits.is_empty());
}

#[test]
fn snapshot_round_trip_preserves_traits_and_state() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("persona.db");
    let agent_id = Uuid::new_v4();
    let store = PersonaStore::open(agent_id, &db, PersonaPersistenceConfig::default()).unwrap();

    let mut persona = make_default_persona(agent_id, "周芷若");
    let mut courage = Trait::new("勇气".to_string(), TraitType::Moral, 50);
    courage.apply_change(
        TraitChange::new("勇气".to_string(), 35, "经历生死搏斗".to_string(), 42),
        42,
    );
    persona.traits.insert("courage".to_string(), courage);
    persona.current_state = stress_state(77);

    store.snapshot(&persona, 42).unwrap();

    let loaded = store
        .load_or_default(make_default_persona(agent_id, "周芷若"))
        .unwrap();
    let stored = loaded.traits.get("courage").expect("courage trait");
    assert_eq!(stored.name, "勇气");
    assert_eq!(stored.value(), 85);
    assert_eq!(loaded.current_state.current_emotion, "焦虑");
    assert_eq!(loaded.current_state.stress_level, 77);
}

#[test]
fn load_or_default_keeps_tier1_tier2_from_input_default() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("persona.db");
    let agent_id = Uuid::new_v4();
    let store = PersonaStore::open(agent_id, &db, PersonaPersistenceConfig::default()).unwrap();

    let mut persona = make_default_persona(agent_id, "令狐冲");
    persona.base_description = "华山派大弟子".to_string();
    let mut traits = HashMap::new();
    let mut kindness = Trait::new("仁心".to_string(), TraitType::Moral, 50);
    kindness.apply_change(
        TraitChange::new("仁心".to_string(), 40, "救人于危难".to_string(), 7),
        7,
    );
    traits.insert("kindness".to_string(), kindness);
    persona.traits = traits;
    persona.current_state = stress_state(30);
    store.snapshot(&persona, 7).unwrap();

    let input_default = {
        let mut p = DynamicPersona::new(agent_id, "令狐冲", "wrong description");
        p.traits.clear();
        p
    };
    let loaded = store.load_or_default(input_default).unwrap();

    assert_eq!(loaded.name, "令狐冲");
    assert_eq!(loaded.base_description, "wrong description");
    let stored = loaded.traits.get("kindness").expect("kindness trait");
    assert_eq!(stored.value(), 90);
    assert_eq!(loaded.current_state.stress_level, 30);
}

#[test]
fn snapshot_now_overwrites_previous_row() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("persona.db");
    let agent_id = Uuid::new_v4();
    let store = PersonaStore::open(agent_id, &db, PersonaPersistenceConfig::default()).unwrap();

    let mut persona = make_default_persona(agent_id, "韦小宝");
    persona.current_state = stress_state(10);
    store.snapshot(&persona, 1).unwrap();

    persona.current_state = stress_state(80);
    store.snapshot_now(&persona, 2).unwrap();

    let loaded = store
        .load_or_default(make_default_persona(agent_id, "韦小宝"))
        .unwrap();
    assert_eq!(loaded.current_state.stress_level, 80);
}

#[test]
fn update_config_changes_snapshot_interval() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("persona.db");
    let agent_id = Uuid::new_v4();
    let store = PersonaStore::open(agent_id, &db, PersonaPersistenceConfig::default()).unwrap();

    assert_eq!(store.config_snapshot_interval(), 10);
    assert!(store.config_flush_on_death());
    assert!(store.config_flush_on_shutdown());

    let new_cfg = PersonaPersistenceConfig {
        snapshot_interval_ticks: 25,
        flush_on_shutdown: false,
        flush_on_death: true,
    };
    store.update_config(new_cfg).unwrap();

    assert_eq!(store.config_snapshot_interval(), 25);
    assert!(!store.config_flush_on_shutdown());
    assert!(store.config_flush_on_death());
}

#[allow(dead_code)]
fn _ensure_thread_safe_persona_compiles() {
    let persona = ThreadSafePersona::new(make_default_persona(Uuid::new_v4(), "test"));
    let _ = persona.read(|p| p.name.clone());
}
