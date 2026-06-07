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
use crate::soul::actor::engine::FALLBACK_NARRATIVE; // 统一降级文本
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

/// 情绪上下文快照（由 lifecycle 每 tick 传入）
#[derive(Debug, Clone)]
pub struct EmotionContext {
    pub valence: f32,
    pub arousal: f32,
    pub emotion_label: String,
    pub encoding_config: crate::component::emotion::config::EncodingConfig,
    pub retrieval_config: crate::component::emotion::config::RetrievalConfig,
}

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
    /// 叙事合成最小事件数（触发 LLM 调用的最低事件数）
    pub narrative_min_events: usize,
}

impl Default for MemoryManagerConfig {
    fn default() -> Self {
        Self {
            agent_id: Uuid::nil(),
            db_dir: PathBuf::from("."),
            working_memory_size: 20,
            episodic_threshold: 0.3,
            ebbinghaus_config: EbbinghausConfig::default(),
            narrative_min_events: 1,
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
    ///
    /// - 所有事件写入工作记忆（始终执行，与叙事合成解耦）
    /// - 高重要性事件（≥ episodic_threshold）经 LLM 叙事合成后写入情景记忆
    /// - cognitive_engine 可选（不可用时降级为原始描述拼接）
    pub async fn process_events(
        &mut self,
        events: &[WorldEvent],
        cognitive_engine: Option<&crate::soul::actor::CognitiveEngine>,
        emotion_ctx: Option<crate::component::memory::manager::EmotionContext>,
    ) -> Result<()> {
        // 1. 计算每个事件的 importance_score（情绪增强或基础评分）
        let scored: Vec<(f32, WorldEvent)> = events
            .iter()
            .map(|e| {
                let score = match &emotion_ctx {
                    Some(ctx) => self.scorer.score_with_emotion(
                        &e.event_type,
                        &e.description,
                        &e.metadata,
                        ctx.arousal,
                        &ctx.encoding_config,
                    ),
                    None => self
                        .scorer
                        .score(&e.event_type, &e.description, &e.metadata),
                };
                (score, e.clone())
            })
            .collect();

        // 2. 所有事件写入工作记忆（含情绪编码字段）
        for (importance, event) in &scored {
            let mut entry = MemoryEntry::new(
                self.config.agent_id,
                event.tick_id,
                event.description.clone(),
            )
            .with_event_type(event.event_type.to_string())
            .with_importance(*importance)
            .with_metadata(event.metadata.clone());

            if let Some(ctx) = &emotion_ctx {
                entry = entry.with_encoding_emotion(
                    ctx.valence,
                    ctx.arousal,
                    ctx.emotion_label.clone(),
                );
            }

            self.working.add(&mut entry).await?;
        }

        // 3. 高重要性事件：叙事合成写入情景记忆
        let significant: Vec<WorldEvent> = scored
            .into_iter()
            .filter(|(importance, _)| *importance >= self.config.episodic_threshold)
            .map(|(_, e)| e)
            .collect();

        if significant.len() >= self.config.narrative_min_events {
            let narrative = if let Some(engine) = cognitive_engine {
                let summary = engine.get_summary_context();
                let outcome = engine.get_outcome_context_public();
                engine
                    .synthesize_memory_narrative(&significant, &summary, &outcome)
                    .await
            } else {
                // cognitive_engine 不可用时：显式警告 + 统一降级文本
                tracing::warn!(
                    "Memory narrative synthesis skipped: cognitive_engine unavailable (agent_id={}, events={})",
                    self.config.agent_id,
                    significant.len()
                );
                FALLBACK_NARRATIVE.to_string()
            };

            let mut entry =
                MemoryEntry::new(self.config.agent_id, significant[0].tick_id, narrative)
                    .with_importance(1.0)
                    .with_event_type("synthesized_memory".to_string());

            let episodic_id = self.episodic.add(&mut entry).await?;
            if episodic_id > 0
                && let Some(ref semantic) = self.semantic
            {
                match semantic.lock().await.add(&mut entry).await {
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(
                            "SemanticMemory::add() failed for tick {}: {}, embedding not stored",
                            significant[0].tick_id,
                            e
                        );
                    }
                }
            }
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

    /// 按时间倒序回忆近期被遗忘的事件（跳过语义搜索）
    pub async fn recall_recent_archived(&self, limit: usize) -> Result<Vec<MemoryEntry>> {
        self.episodic.search_archived("", limit).await
    }

    /// 构建 LLM 上下文
    ///
    /// 只展示 episodic 记忆（LLM 叙事摘要），不暴露 working memory 流水账。
    /// working memory 保留供 outcome memory / summary window 等内部使用。
    pub async fn build_llm_context(&self) -> String {
        self.build_llm_context_with_emotion(None).await
    }

    /// 构建 LLM 上下文（含效价一致性检索偏置）
    pub async fn build_llm_context_with_emotion(
        &self,
        emotion_ctx: Option<&EmotionContext>,
    ) -> String {
        let episodic_memories = match emotion_ctx {
            Some(ctx) => self
                .episodic
                .get_top_by_importance_with_bias(10, ctx.valence, &ctx.retrieval_config)
                .await
                .unwrap_or_default(),
            None => self
                .episodic
                .get_top_by_importance(10)
                .await
                .unwrap_or_default(),
        };

        let episodic_summary = if episodic_memories.is_empty() {
            return String::new();
        } else {
            episodic_memories
                .iter()
                .enumerate()
                .map(|(i, m)| format!("{}. [Tick {}] {}", i + 1, m.tick_id, m.content,))
                .collect::<Vec<_>>()
                .join("\n")
        };

        format!("### 近期记忆\n{}", episodic_summary)
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
        assert_eq!(config.episodic_threshold, 0.3);
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

        manager.process_events(&events, None, None).await.unwrap();

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

        // SystemNotification importance=0.2 < threshold=0.3 → 不进入 episodic
        let low_events = vec![WorldEvent {
            event_type: WorldEventType::SystemNotification,
            tick_id: 1,
            description: "你吃了馒头".to_string(),
            metadata: json!({}),
        }];
        manager
            .process_events(&low_events, None, None)
            .await
            .unwrap();

        // 无 episodic 记忆时应返回空字符串（不再展示 working memory 流水账）
        let context = manager.build_llm_context().await;
        assert!(context.is_empty(), "低价值事件不应出现在 LLM 上下文中");

        // PublicMessage importance=0.7 ≥ threshold=0.3 → 进入 episodic
        let high_events = vec![WorldEvent {
            event_type: WorldEventType::PublicMessage,
            tick_id: 2,
            description: "张三向你问好".to_string(),
            metadata: json!({}),
        }];
        manager
            .process_events(&high_events, None, None)
            .await
            .unwrap();

        let context = manager.build_llm_context().await;
        assert!(
            !context.is_empty(),
            "高价值事件应产生 episodic 记忆并出现在 LLM 上下文中"
        );
    }
}
