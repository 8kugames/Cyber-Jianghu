// ============================================================================
// 记忆管理器
// ============================================================================
// 设计文档: (项目根)/docs/superpowers/specs/2025-03-15-semantic-memory-and-forgetting-design.md
//
// 协调所有记忆后端，提供统一的记忆管理接口
// - WorkingMemory: RAM FIFO 队列
// - EpisodicMemory: SQLite 持久化 + 遗忘（is_archived 标记归档）
// - SemanticMemory: 向量检索 + FTS 降级
// ============================================================================

use crate::component::memory::backend::{
    ForgettableBackend, MemoryBackend, SearchableBackend, SemanticSearchable,
};
use crate::component::memory::backends::episodic::EpisodicMemoryBackend;
use crate::component::memory::backends::semantic::SemanticMemoryBackend;
use crate::component::memory::backends::working::WorkingMemoryBackend;
use crate::component::memory::embedder::EmbedderService;
use crate::component::memory::forgetting::ForgettingScheduler;
use crate::component::memory::scorer::ImportanceScorer;
use crate::component::memory::types::{EbbinghausConfig, ForgettingReport, MemoryEntry};
use crate::models::WorldEvent;
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

/// 记忆管理器配置
#[derive(Debug, Clone)]
pub struct MemoryManagerConfig {
    /// Agent ID
    pub agent_id: Uuid,
    /// 数据库目录
    pub db_dir: PathBuf,
    /// 工作记忆大小
    pub working_memory_size: usize,
    /// 情景记忆保存阈值
    pub episodic_threshold: f32,
    /// 艾宾浩斯配置
    pub ebbinghaus_config: EbbinghausConfig,
}

impl Default for MemoryManagerConfig {
    fn default() -> Self {
        Self {
            agent_id: Uuid::nil(),
            db_dir: PathBuf::from("."),
            working_memory_size: 20,
            episodic_threshold: 0.5,
            ebbinghaus_config: EbbinghausConfig::default(),
        }
    }
}

/// 记忆管理器
///
/// 协调所有记忆后端，提供统一的记忆管理接口
pub struct MemoryManager {
    /// 配置
    config: MemoryManagerConfig,
    /// 工作记忆后端
    working: WorkingMemoryBackend,
    /// 情景记忆后端
    episodic: EpisodicMemoryBackend,
    /// 语义记忆后端（可选，使用本地 embedder）
    semantic: Option<Arc<tokio::sync::Mutex<SemanticMemoryBackend>>>,
    /// 遗忘调度器
    forgetting_scheduler: ForgettingScheduler,
    /// 重要性评分器
    scorer: ImportanceScorer,
}

impl MemoryManager {
    /// 创建新的记忆管理器
    pub fn new(config: MemoryManagerConfig) -> Result<Self> {
        // 确保目录存在
        std::fs::create_dir_all(&config.db_dir).context("Failed to create database directory")?;

        // 创建各后端
        let working = WorkingMemoryBackend::new(config.working_memory_size);
        let episodic = EpisodicMemoryBackend::new(config.agent_id, &config.db_dir)
            .context("Failed to create episodic memory backend")?;

        // 初始化语义记忆（使用本地 embedder）
        let embedder = Arc::new(EmbedderService::new());
        let episodic_db_path = config.db_dir.join(format!("agent_{}.db", config.agent_id));
        let semantic_config =
            crate::component::memory::backends::semantic::backend::SemanticMemoryConfig {
                dimension: 512,
                db_path: config.db_dir.join("semantic.db"),
                episodic_db_path,
                embedding_threshold: 0.7,
            };

        let semantic = match SemanticMemoryBackend::new(config.agent_id, semantic_config, embedder)
        {
            Ok(backend) => Some(Arc::new(tokio::sync::Mutex::new(backend))),
            Err(e) => {
                tracing::warn!("Failed to initialize semantic memory: {}", e);
                None
            }
        };

        // 创建遗忘调度器
        let forgetting_scheduler = ForgettingScheduler::new(config.ebbinghaus_config.clone(), 0);

        Ok(Self {
            config,
            working,
            episodic,
            semantic,
            forgetting_scheduler,
            scorer: ImportanceScorer::new(),
        })
    }

    /// 处理世界事件
    pub async fn process_events(&mut self, events: &[WorldEvent]) -> Result<()> {
        for event in events {
            // 计算重要性评分
            let importance =
                self.scorer
                    .score(&event.event_type, &event.description, &event.metadata);

            // 创建记忆条目
            let entry = MemoryEntry::new(
                self.config.agent_id,
                event.tick_id,
                event.description.clone(),
            )
            .with_event_type(event.event_type.to_string())
            .with_importance(importance)
            .with_metadata(event.metadata.clone());

            // 添加到工作记忆
            self.working.add(entry.clone()).await?;

            // 添加到情景记忆（会根据阈值过滤）
            self.episodic.add(entry).await?;
        }

        Ok(())
    }

    /// 执行遗忘机制
    ///
    /// 应该每 84 tick 调用一次
    pub async fn run_forgetting(&mut self, current_tick: i64) -> Result<ForgettingReport> {
        if !self.forgetting_scheduler.should_run(current_tick) {
            return Ok(ForgettingReport {
                checked_count: 0,
                archived_count: 0,
                retained_count: 0,
            });
        }

        // 获取所有记忆
        let all_memories = self.episodic.get_recent(10000).await?;
        let checked_count = all_memories.len();

        // 计算需要归档的记忆 ID
        let mut to_archive_ids = Vec::new();
        let mut retained_count = 0;

        for memory in &all_memories {
            let retention = self
                .forgetting_scheduler
                .calculate_retention(memory, current_tick);

            if retention < self.forgetting_scheduler.config().retention_threshold {
                if let Some(id) = memory.id {
                    to_archive_ids.push(id);
                }
            } else {
                retained_count += 1;
            }
        }

        // 批量归档（is_archived 标记）
        let archived_count = if !to_archive_ids.is_empty() {
            self.episodic
                .archive_memories(&to_archive_ids)
                .await
                .unwrap_or(0)
        } else {
            0
        };

        // 衰减所有未归档记忆的强度
        if let Err(e) = self.episodic.decay_strength().await {
            tracing::warn!("Memory strength decay failed: {}", e);
        }

        // 标记已执行
        self.forgetting_scheduler.mark_executed(current_tick);

        Ok(ForgettingReport {
            checked_count,
            archived_count,
            retained_count,
        })
    }

    /// 搜索归档记忆（"努力回忆"）
    pub async fn recall_archived(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        // 先尝试语义检索（如果可用）
        if let Some(semantic) = &self.semantic {
            let mut semantic = semantic.lock().await;
            match semantic.search_similar(query, limit).await {
                Ok(results) if !results.is_empty() => {
                    return Ok(results);
                }
                _ => {} // 失败或为空则降级
            }
        }

        // 降级到情景记忆的归档检索（is_archived=TRUE）
        self.episodic.search_archived(query, limit).await
    }

    /// 构建 LLM 上下文
    pub async fn build_llm_context(&self) -> String {
        let working_context = self.working.build_context();

        // 获取重要的情景记忆
        let episodic_memories = self
            .episodic
            .get_top_by_importance(10)
            .await
            .unwrap_or_default();

        let episodic_summary = if episodic_memories.is_empty() {
            "暂无重要记忆".to_string()
        } else {
            episodic_memories
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    format!(
                        "{}. [Tick {}] {} (重要性: {:.1})",
                        i + 1,
                        m.tick_id,
                        m.content,
                        m.importance_score
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        format!(
            "# 最近事件（工作记忆）\n{}\n\n# 重要记忆（情景记忆）\n{}",
            working_context, episodic_summary
        )
    }

    /// 获取记忆统计
    pub async fn stats(&self) -> MemoryManagerStats {
        MemoryManagerStats {
            working_count: self.working.count().await.unwrap_or(0),
            episodic_count: self.episodic.count().await.unwrap_or(0),
            archive_count: self.episodic.archived_count().await.unwrap_or(0),
        }
    }

    /// 清空所有记忆
    pub async fn clear_all(&mut self) -> Result<()> {
        MemoryBackend::clear(&mut self.working).await?;
        MemoryBackend::clear(&mut self.episodic).await?;
        Ok(())
    }

    /// 获取工作记忆
    pub fn working(&self) -> &WorkingMemoryBackend {
        &self.working
    }

    /// 获取工作记忆的可变引用
    pub fn working_mut(&mut self) -> &mut WorkingMemoryBackend {
        &mut self.working
    }

    /// 获取情景记忆
    pub fn episodic(&self) -> &EpisodicMemoryBackend {
        &self.episodic
    }

    /// 获取情景记忆的可变引用
    pub fn episodic_mut(&mut self) -> &mut EpisodicMemoryBackend {
        &mut self.episodic
    }

    /// 获取 Agent ID
    pub fn agent_id(&self) -> Uuid {
        self.config.agent_id
    }
}

/// 记忆管理器统计信息
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryManagerStats {
    /// 工作记忆数量
    pub working_count: usize,
    /// 情景记忆数量
    pub episodic_count: usize,
    /// 归档记忆数量
    pub archive_count: usize,
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::WorldEventType;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn test_config_default() {
        let config = MemoryManagerConfig::default();
        assert_eq!(config.working_memory_size, 20);
        assert_eq!(config.episodic_threshold, 0.5);
    }

    #[tokio::test]
    async fn test_memory_manager_creation() {
        let temp_dir = TempDir::new().unwrap();
        let config = MemoryManagerConfig {
            agent_id: Uuid::new_v4(),
            db_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let manager = MemoryManager::new(config).unwrap();
        let stats = manager.stats().await;

        assert_eq!(stats.working_count, 0);
        assert_eq!(stats.episodic_count, 0);
    }

    #[tokio::test]
    async fn test_process_events() {
        let temp_dir = TempDir::new().unwrap();
        let config = MemoryManagerConfig {
            agent_id: Uuid::new_v4(),
            db_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let mut manager = MemoryManager::new(config).unwrap();

        let events = vec![
            WorldEvent {
                event_type: WorldEventType::DeathNotification,
                tick_id: 1,
                description: "你受到了10点伤害".to_string(),
                metadata: json!({"hp_delta": -10}),
            },
            WorldEvent {
                event_type: WorldEventType::SystemNotification,
                tick_id: 1,
                description: "你休息了一会".to_string(),
                metadata: json!({}),
            },
        ];

        manager.process_events(&events).await.unwrap();

        let stats = manager.stats().await;
        assert_eq!(stats.working_count, 2);
        assert!(stats.episodic_count >= 1); // 至少有一个高重要性记忆
    }

    #[tokio::test]
    async fn test_build_llm_context() {
        let temp_dir = TempDir::new().unwrap();
        let config = MemoryManagerConfig {
            agent_id: Uuid::new_v4(),
            db_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let mut manager = MemoryManager::new(config).unwrap();

        let events = vec![WorldEvent {
            event_type: WorldEventType::SystemNotification,
            tick_id: 1,
            description: "你吃了馒头".to_string(),
            metadata: json!({}),
        }];

        manager.process_events(&events).await.unwrap();

        let context = manager.build_llm_context().await;
        assert!(context.contains("你吃了馒头"));
        assert!(context.contains("最近事件"));
    }
}
