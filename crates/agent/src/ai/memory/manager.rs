// ============================================================================
// 记忆管理器
// ============================================================================
// 设计文档: (项目根)/docs/superpowers/specs/2025-03-15-semantic-memory-and-forgetting-design.md
//
// 协调所有记忆后端，提供统一的记忆管理接口
// - WorkingMemory: RAM FIFO 队列
// - EpisodicMemory: SQLite 持久化 + 遗忘
// - SemanticMemory: 向量检索 + FTS 降级
// - ArchiveMemory: 遗忘归档存储
// ============================================================================

use crate::ai::llm::LlmClient;
use crate::ai::memory::backend::{MemoryBackend, SearchableBackend, SemanticSearchable};
use crate::ai::memory::backends::archive::ArchiveMemoryBackend;
use crate::ai::memory::backends::episodic::EpisodicMemoryBackend;
use crate::ai::memory::backends::semantic::SemanticMemoryBackend;
use crate::ai::memory::backends::working::WorkingMemoryBackend;
use crate::ai::memory::embedder::EmbedderService;
use crate::ai::memory::forgetting::ForgettingScheduler;
use crate::ai::memory::scorer::ImportanceScorer;
use crate::ai::memory::types::{EbbinghausConfig, ForgettingReport, MemoryEntry};
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
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            agent_id: Uuid::nil(),
            db_dir: home.join(".cyber-jianghu").join("data"),
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
    /// 归档记忆后端
    archive: ArchiveMemoryBackend,
    /// 语义记忆后端（可选，需要 LlmClient）
    semantic: Option<Arc<tokio::sync::Mutex<SemanticMemoryBackend>>>,
    /// 遗忘调度器
    forgetting_scheduler: ForgettingScheduler,
    /// 重要性评分器
    scorer: ImportanceScorer,
}

impl MemoryManager {
    /// 创建新的记忆管理器（带 LlmClient）
    pub fn new_with_llm(
        config: MemoryManagerConfig,
        llm_client: Arc<dyn LlmClient>,
    ) -> Result<Self> {
        // 确保目录存在
        std::fs::create_dir_all(&config.db_dir).context("Failed to create database directory")?;

        // 创建各后端
        let working = WorkingMemoryBackend::new(config.working_memory_size);
        let episodic = EpisodicMemoryBackend::new(config.agent_id, &config.db_dir)
            .context("Failed to create episodic memory backend")?;
        let archive = ArchiveMemoryBackend::new(config.agent_id, &config.db_dir)
            .context("Failed to create archive memory backend")?;

        // 初始化语义记忆
        let embedder = Arc::new(EmbedderService::new(Some(llm_client.clone())));
        let episodic_db_path = config.db_dir.join(format!("agent_{}.db", config.agent_id));
        let semantic_config =
            crate::ai::memory::backends::semantic::backend::SemanticMemoryConfig {
                db_path: config.db_dir.join("semantic.db"),
                episodic_db_path,
                ..Default::default()
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
            archive,
            semantic,
            forgetting_scheduler,
            scorer: ImportanceScorer::new(),
        })
    }

    /// 创建新的记忆管理器（无 LLM）
    pub fn new(config: MemoryManagerConfig) -> Result<Self> {
        // 确保目录存在
        std::fs::create_dir_all(&config.db_dir).context("Failed to create database directory")?;

        // 创建各后端
        let working = WorkingMemoryBackend::new(config.working_memory_size);
        let episodic = EpisodicMemoryBackend::new(config.agent_id, &config.db_dir)
            .context("Failed to create episodic memory backend")?;
        let archive = ArchiveMemoryBackend::new(config.agent_id, &config.db_dir)
            .context("Failed to create archive memory backend")?;

        // 创建遗忘调度器
        let forgetting_scheduler = ForgettingScheduler::new(config.ebbinghaus_config.clone(), 0);

        Ok(Self {
            config,
            working,
            episodic,
            archive,
            semantic: None,
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
            .with_event_type(event.event_type.clone())
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

        // 计算需要归档的记忆
        let mut to_archive = Vec::new();
        let mut retained_count = 0;

        for memory in &all_memories {
            let retention = self
                .forgetting_scheduler
                .calculate_retention(memory, current_tick);

            if retention < self.forgetting_scheduler.config().retention_threshold {
                to_archive.push(memory.clone());
            } else {
                retained_count += 1;
            }
        }

        // 归档记忆
        let archived_count = to_archive.len();
        for memory in &to_archive {
            if let Some(id) = memory.id {
                self.archive.add(memory.clone()).await?;
                // 标记情景记忆为已归档（需要 EpisodicMemoryBackend 支持）
                let _ = id; // 避免 unused warning
            }
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

        // 降级到归档检索
        self.archive.search(query, limit)
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
            archive_count: self.archive.count().await.unwrap_or(0),
        }
    }

    /// 清空所有记忆
    pub async fn clear_all(&mut self) -> Result<()> {
        MemoryBackend::clear(&mut self.working).await?;
        MemoryBackend::clear(&mut self.episodic).await?;
        MemoryBackend::clear(&mut self.archive).await?;
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

    /// 获取归档记忆
    pub fn archive(&self) -> &ArchiveMemoryBackend {
        &self.archive
    }

    /// 获取归档记忆的可变引用
    pub fn archive_mut(&mut self) -> &mut ArchiveMemoryBackend {
        &mut self.archive
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
                event_type: "combat".to_string(),
                tick_id: 1,
                description: "你受到了10点伤害".to_string(),
                metadata: json!({"hp_delta": -10}),
            },
            WorldEvent {
                event_type: "routine".to_string(),
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
            event_type: "routine".to_string(),
            tick_id: 1,
            description: "你吃了馒头".to_string(),
            metadata: json!({}),
        }];

        manager.process_events(&events).await.unwrap();

        let context = manager.build_llm_context().await;
        assert!(context.contains("你吃了馒头"));
        assert!(context.contains("最近事件"));
    }

    #[test]
    fn test_memory_manager_stats() {
        let stats = MemoryManagerStats {
            working_count: 10,
            episodic_count: 50,
            archive_count: 5,
        };

        assert_eq!(stats.working_count, 10);
        assert_eq!(stats.episodic_count, 50);
        assert_eq!(stats.archive_count, 5);
    }
}
