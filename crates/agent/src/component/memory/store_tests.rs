//! store 模块单测（自 store.rs 外移，内容未改）

use super::MemoryStore;

use super::*;
use tempfile::TempDir;

#[test]
fn test_memory_store_init() {
    let temp_dir = TempDir::new().unwrap();
    let agent_id = Uuid::new_v4();

    let store = MemoryStore::new(agent_id, temp_dir.path()).unwrap();

    assert_eq!(store.agent_id(), agent_id);
    assert!(store.db_path().exists());
    assert_eq!(store.count().unwrap(), 0);
}

#[test]
fn test_add_and_retrieve_memory() {
    let temp_dir = TempDir::new().unwrap();
    let agent_id = Uuid::new_v4();
    let store = MemoryStore::new(agent_id, temp_dir.path()).unwrap();

    let memory = ClientMemory::new(agent_id, 1, "测试记忆".to_string())
        .with_importance(0.8)
        .with_type("test".to_string());

    let id = store.add_memory(&memory).unwrap();
    assert!(id > 0);

    let memories = store.get_top_memories(10).unwrap();
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0].content, "测试记忆");
    assert_eq!(memories[0].importance_score, 0.8);
}

#[test]
fn test_batch_insert() {
    let temp_dir = TempDir::new().unwrap();
    let agent_id = Uuid::new_v4();
    let store = MemoryStore::new(agent_id, temp_dir.path()).unwrap();

    let memories: Vec<ClientMemory> = (1..=10)
        .map(|i| ClientMemory::new(agent_id, i, format!("记忆 {}", i)))
        .collect();

    store.add_memories_batch(&memories).unwrap();
    assert_eq!(store.count().unwrap(), 10);
}

#[test]
fn test_cleanup_old_memories() {
    let temp_dir = TempDir::new().unwrap();
    let agent_id = Uuid::new_v4();
    let store = MemoryStore::new(agent_id, temp_dir.path()).unwrap();

    // 添加 10 条记忆
    for i in 1..=10 {
        let memory = ClientMemory::new(agent_id, i, format!("记忆 {}", i));
        store.add_memory(&memory).unwrap();
    }

    assert_eq!(store.count().unwrap(), 10);

    // 清理，只保留最近 5 条
    let cleaned = store.cleanup_old_memories(5).unwrap();
    assert_eq!(cleaned, 5);
    assert_eq!(store.count().unwrap(), 5);
}

#[test]
fn test_get_top_memories_excluding_types() {
    let temp_dir = TempDir::new().unwrap();
    let agent_id = Uuid::new_v4();
    let store = MemoryStore::new(agent_id, temp_dir.path()).unwrap();

    let summary = ClientMemory::new(agent_id, 1, "昨日日记".to_string())
        .with_importance(0.8)
        .with_type("daily_summary".to_string());
    let stats = ClientMemory::new(agent_id, 2, "动作统计".to_string())
        .with_importance(0.8)
        .with_type("daily_action_stats".to_string());
    let lived = ClientMemory::new(agent_id, 3, "真实体验".to_string())
        .with_importance(0.7)
        .with_type("action_result".to_string());
    store.add_memory(&summary).unwrap();
    store.add_memory(&stats).unwrap();
    store.add_memory(&lived).unwrap();

    let top = store
        .get_top_memories_excluding_types(20, &["daily_summary", "daily_action_stats"])
        .unwrap();
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].event_type, "action_result");

    // 空排除列表退化为普通 top-K
    let all = store.get_top_memories_excluding_types(20, &[]).unwrap();
    assert_eq!(all.len(), 3);
}

/// 回归：SELECT * 的列序由建表+迁移历史决定（embedding BLOB 落位在
/// encoding_valence 之前），row_to_memory 必须按列名取值。
/// embedding 与 encoding 字段同时非 NULL 时，固定下标映射既会报
/// Invalid column type，也会静默错位读错列。
#[test]
fn test_read_memory_with_embedding_and_encoding() {
    let temp_dir = TempDir::new().unwrap();
    let agent_id = Uuid::new_v4();
    let store = MemoryStore::new(agent_id, temp_dir.path()).unwrap();

    let mut memory = ClientMemory::new(agent_id, 1, "带向量记忆".to_string());
    memory.encoding_valence = Some(0.6);
    memory.encoding_arousal = Some(-0.2);
    memory.encoding_emotion = Some("joy".to_string());
    let id = store.add_memory(&memory).unwrap();
    store.update_embedding(id, &[1u8, 2, 3]).unwrap();

    let got = store.get_recent_memories(10).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].content, "带向量记忆");
    assert_eq!(got[0].encoding_valence, Some(0.6));
    assert_eq!(got[0].encoding_arousal, Some(-0.2));
    assert_eq!(got[0].encoding_emotion.as_deref(), Some("joy"));
}

#[test]
fn test_delete_memories_by_type() {
    let temp_dir = TempDir::new().unwrap();
    let agent_id = Uuid::new_v4();
    let store = MemoryStore::new(agent_id, temp_dir.path()).unwrap();

    for i in 0..2 {
        let m = ClientMemory::new(agent_id, i, format!("统计 {}", i))
            .with_importance(0.8)
            .with_type("daily_action_stats".to_string());
        store.add_memory(&m).unwrap();
    }
    let keep = ClientMemory::new(agent_id, 10, "保留".to_string())
        .with_importance(0.6)
        .with_type("action_result".to_string());
    store.add_memory(&keep).unwrap();

    let deleted = store.delete_memories_by_type("daily_action_stats").unwrap();
    assert_eq!(deleted, 2);
    assert_eq!(store.count().unwrap(), 1);
    // 幂等：再次删除返回 0
    assert_eq!(
        store.delete_memories_by_type("daily_action_stats").unwrap(),
        0
    );
}
