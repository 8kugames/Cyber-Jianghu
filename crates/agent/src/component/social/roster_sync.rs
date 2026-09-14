// ============================================================================
// 关系名册同步：初遇登记 + 名称跟随
// ============================================================================
//
// RelationshipStore 的扩展 impl（Rust 允许同 crate 跨文件固有 impl）。
// 独立成文件的唯一原因：relationship.rs 已越过项目 800 行上限约束（AGENTS.md），
// 本文件承载 triple-review F1 要求的机械迁移，逻辑与数据归属不变；
// seam 账本 agent-relationship-roster-sync 的 single_writer 即本文件。
// ============================================================================

use anyhow::Result;
use uuid::Uuid;

use super::relationship::RelationshipStore;
use crate::component::social::relationship_types::{KeyEvent, RelationshipMemory};

impl RelationshipStore {
    /// 逐 tick 关系名册同步：初遇登记 + 名称跟随
    ///
    /// - 既有关系缺失 → 登记"初遇"相识记录（好感度 0 + 一条初遇关键事件），
    ///   关系页不再依赖"必须发生社交事件且 LLM 评估成功"才建档；
    /// - 既有关系存在且实体名非泛化 → 名称对齐服务器权威名（对方改名自愈）。
    ///
    /// 幂等：记录已存在且名称一致时不产生任何写入。
    pub fn sync_roster(&self, entities: &[(Uuid, String)], tick_id: i64) -> Result<()> {
        for (target_id, name) in entities {
            if name.is_empty() {
                continue;
            }
            match self.get_relationship(*target_id)? {
                None => {
                    let mut memory = RelationshipMemory::new(*target_id, name.clone());
                    memory.update_interaction(tick_id);
                    memory.add_event(KeyEvent::new(
                        tick_id,
                        "初遇",
                        format!("初见于江湖，与{name}相识"),
                        0,
                    ));
                    self.upsert_relationship(&memory)?;
                }
                Some(mut existing) => {
                    if !is_generic_name(name) && existing.target_name != *name {
                        existing.target_name = name.clone();
                        existing.updated_at = chrono::Utc::now();
                        self.upsert_relationship(&existing)?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// 泛化名判定：空名、"陌生人"、server 缺名回退（"Agent-{uuid}"）
///
/// 泛化名只可作初遇占位，不得覆写已知的真名。
fn is_generic_name(name: &str) -> bool {
    name.is_empty() || name == "陌生人" || name.starts_with("Agent-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_sync_roster_creates_acquaintance_on_first_sight() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("relationships.db");
        let store = RelationshipStore::open(Uuid::new_v4(), &db_path).unwrap();
        let target_id = Uuid::new_v4();

        store
            .sync_roster(&[(target_id, "温九辞".to_string())], 42)
            .unwrap();

        let rel = store.get_relationship(target_id).unwrap().unwrap();
        assert_eq!(rel.target_name, "温九辞");
        assert_eq!(rel.favorability, 0);
        assert_eq!(rel.last_interaction_tick, 42);
        assert_eq!(rel.key_events.len(), 1);
        assert_eq!(rel.key_events[0].event_type, "初遇");
    }

    #[test]
    fn test_sync_roster_idempotent_no_duplicate_events() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("relationships.db");
        let store = RelationshipStore::open(Uuid::new_v4(), &db_path).unwrap();
        let target_id = Uuid::new_v4();

        store
            .sync_roster(&[(target_id, "温九辞".to_string())], 1)
            .unwrap();
        store
            .sync_roster(&[(target_id, "温九辞".to_string())], 2)
            .unwrap();

        let rel = store.get_relationship(target_id).unwrap().unwrap();
        assert_eq!(rel.key_events.len(), 1, "重复同步不得产生重复初遇事件");
    }

    #[test]
    fn test_sync_roster_follows_rename_and_preserves_data() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("relationships.db");
        let store = RelationshipStore::open(Uuid::new_v4(), &db_path).unwrap();
        let target_id = Uuid::new_v4();

        // 先有社交事件建档（好感度 +5）
        store
            .record_social_event(target_id, "温九辞", 5, "speak", "初次交谈", 5)
            .unwrap();
        // 对方改名 → 名册同步跟随
        store
            .sync_roster(&[(target_id, "温大夫".to_string())], 9)
            .unwrap();

        let rel = store.get_relationship(target_id).unwrap().unwrap();
        assert_eq!(rel.target_name, "温大夫", "改名应在下一认知 tick 自愈");
        assert_eq!(rel.favorability, 5, "改名不得影响好感度");
        assert_eq!(rel.key_events.len(), 1, "改名不得产生新事件");
        assert_eq!(rel.key_events[0].event_type, "speak", "既有事件必须保留");
    }

    #[test]
    fn test_sync_roster_generic_name_never_overwrites_real_name() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("relationships.db");
        let store = RelationshipStore::open(Uuid::new_v4(), &db_path).unwrap();
        let target_id = Uuid::new_v4();

        store
            .record_social_event(target_id, "温九辞", 5, "speak", "交谈", 0)
            .unwrap();
        // server 缺名回退（Agent- 前缀）不得覆写真名
        store
            .sync_roster(&[(target_id, "Agent-8f2a1b3c".to_string())], 6)
            .unwrap();

        let rel = store.get_relationship(target_id).unwrap().unwrap();
        assert_eq!(rel.target_name, "温九辞");
    }
}
