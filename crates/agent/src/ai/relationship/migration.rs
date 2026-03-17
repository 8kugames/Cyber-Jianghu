// ============================================================================
// 数据迁移模块
// ============================================================================
//
// 为现有关系记录迁移 AI 生成的叙事化描述

use anyhow::Result;
use std::time::Duration;
use crate::ai::relationship::{RelationshipStore, narrative::NarrativeGenerator};
use crate::ai::persona::DynamicPersona;

/// 迁移报告
#[derive(Debug, Clone)]
pub struct MigrationReport {
    pub total: usize,
    pub migrated: usize,
    pub failed: usize,
    pub skipped: usize,
}

impl std::fmt::Display for MigrationReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "迁移完成: 总计 {}，成功 {}，失败 {}，跳过 {}",
            self.total, self.migrated, self.failed, self.skipped
        )
    }
}

/// 迁移现有关系记录，为空描述生成初始值
///
/// 执行时机：Agent 启动时检测到 `self_description` 为空的记录
/// 批量大小：每次处理 10 条，避免过载
/// 失败处理：单条失败不影响其他记录，记录日志
pub async fn migrate_relationship_descriptions(
    store: &RelationshipStore,
    generator: &NarrativeGenerator,
    persona: &DynamicPersona,
) -> Result<MigrationReport> {
    let all_relationships = store.get_all_relationships()?;

    let mut migrated = 0;
    let mut failed = 0;
    let mut skipped = 0;

    // 批量处理
    for memory in all_relationships {
        if !memory.self_description.is_empty() {
            skipped += 1;
            continue;
        }

        match generator.generate_description(&memory, persona).await {
            Ok(description) => {
                if let Err(e) = store.update_self_description(
                    memory.target_agent_id,
                    &description,
                    0
                ) {
                    tracing::warn!("[migration] 更新失败 {}: {}", memory.target_name, e);
                    failed += 1;
                } else {
                    migrated += 1;
                    tracing::info!("[migration] 迁移成功: {} -> {}", memory.target_name, description);
                }
            }
            Err(e) => {
                tracing::warn!("[migration] 生成描述失败 {}: {}", memory.target_name, e);
                failed += 1;
            }
        }

        // 批量间隔，避免过载
        if migrated % 10 == 0 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    Ok(MigrationReport {
        total: migrated + failed + skipped,
        migrated,
        failed,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use uuid::Uuid;
    use crate::ai::llm::MockLlmClient;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_migration_empty_database() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_migration.db");
        let agent_id = Uuid::new_v4();

        let store = RelationshipStore::open(agent_id, &db_path).unwrap();
        let client = MockLlmClient::with_response("测试描述");
        let generator = NarrativeGenerator::new(Arc::new(client));
        let persona = DynamicPersona::new(agent_id, "测试", "一个测试角色");

        let report = migrate_relationship_descriptions(&store, &generator, &persona).await.unwrap();

        assert_eq!(report.total, 0);
        assert_eq!(report.migrated, 0);
        assert_eq!(report.failed, 0);
        assert_eq!(report.skipped, 0);
    }

    #[tokio::test]
    async fn test_migration_with_existing_descriptions() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_migration_skip.db");
        let agent_id = Uuid::new_v4();

        let store = RelationshipStore::open(agent_id, &db_path).unwrap();
        let client = MockLlmClient::with_response("新描述");
        let generator = NarrativeGenerator::new(Arc::new(client));
        let persona = DynamicPersona::new(agent_id, "测试", "一个测试角色");

        // 添加一个已有描述的关系
        let mut memory = crate::ai::relationship::types::RelationshipMemory::new(Uuid::new_v4(), "已有描述的目标");
        memory.self_description = "已有描述".to_string();
        store.upsert_relationship(&memory).unwrap();

        let report = migrate_relationship_descriptions(&store, &generator, &persona).await.unwrap();

        assert_eq!(report.total, 1);
        assert_eq!(report.migrated, 0);
        assert_eq!(report.failed, 0);
        assert_eq!(report.skipped, 1);
    }

    #[tokio::test]
    async fn test_migration_generates_descriptions() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_migration_generate.db");
        let agent_id = Uuid::new_v4();

        let store = RelationshipStore::open(agent_id, &db_path).unwrap();
        let client = MockLlmClient::with_response("AI生成的描述");
        let generator = NarrativeGenerator::new(Arc::new(client));
        let persona = DynamicPersona::new(agent_id, "测试", "一个测试角色");

        // 添加一个没有描述的关系
        let memory = crate::ai::relationship::types::RelationshipMemory::new(Uuid::new_v4(), "需要迁移的目标");
        store.upsert_relationship(&memory).unwrap();

        let report = migrate_relationship_descriptions(&store, &generator, &persona).await.unwrap();

        assert_eq!(report.total, 1);
        assert_eq!(report.migrated, 1);
        assert_eq!(report.failed, 0);
        assert_eq!(report.skipped, 0);

        // 验证描述已保存
        let retrieved = store.get_relationship(memory.target_agent_id).unwrap().unwrap();
        assert_eq!(retrieved.self_description, "AI生成的描述");
    }
}
