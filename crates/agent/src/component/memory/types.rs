// ============================================================================
// 记忆系统公共类型
// ============================================================================
// 设计文档: (项目根)/docs/superpowers/specs/2025-03-15-semantic-memory-and-forgetting-design.md

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// 统一记忆条目 - 所有后端共用
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryEntry {
    /// 记忆 ID（数据库自增）
    pub id: Option<i64>,
    /// Agent ID
    pub agent_id: Uuid,
    /// Tick 编号
    pub tick_id: i64,
    /// 事件类型
    pub event_type: String,
    /// 事件内容（自然语言）
    pub content: String,
    /// 元数据（JSON 格式）
    pub metadata: Value,
    /// 重要性评分 (0.0-1.0)
    pub importance_score: f32,
    /// 记忆强度（用于遗忘计算）
    pub strength: f32,
    /// 嵌入向量 (512维，按需生成)
    pub embedding: Option<Vec<f32>>,
    /// 最后访问时间
    pub last_accessed_at: Option<DateTime<Utc>>,
    /// 访问次数
    pub access_count: u32,
    /// 是否已归档
    pub is_archived: bool,
    /// 编码时的效价（用于检索偏置）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding_valence: Option<f32>,
    /// 编码时的唤醒度（用于编码强度回溯）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding_arousal: Option<f32>,
    /// 编码时的具体情绪标签
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding_emotion: Option<String>,
    /// 创建时间
    pub created_at: DateTime<Utc>,
}

impl MemoryEntry {
    /// 创建新的记忆条目
    pub fn new(agent_id: Uuid, tick_id: i64, content: String) -> Self {
        let now = Utc::now();
        Self {
            id: None,
            agent_id,
            tick_id,
            event_type: "unknown".to_string(),
            content,
            metadata: Value::Null,
            importance_score: 0.5,
            strength: 0.5,
            embedding: None,
            last_accessed_at: None,
            access_count: 0,
            is_archived: false,
            encoding_valence: None,
            encoding_arousal: None,
            encoding_emotion: None,
            created_at: now,
        }
    }

    /// 设置事件类型
    pub fn with_event_type(mut self, event_type: String) -> Self {
        self.event_type = event_type;
        self
    }

    /// 设置重要性评分（同时设置初始强度）
    pub fn with_importance(mut self, importance: f32) -> Self {
        self.importance_score = importance;
        self.strength = importance;
        self
    }

    /// 设置元数据
    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = metadata;
        self
    }

    /// 设置嵌入向量
    pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = Some(embedding);
        self
    }

    /// 设置编码时的情绪信息
    pub fn with_encoding_emotion(mut self, valence: f32, arousal: f32, emotion: String) -> Self {
        self.encoding_valence = Some(valence);
        self.encoding_arousal = Some(arousal);
        self.encoding_emotion = Some(emotion);
        self
    }

    /// 检查是否需要生成向量（按需策略）
    pub fn needs_embedding(&self) -> bool {
        self.embedding.is_none() && self.importance_score >= 0.7 && !self.is_archived
    }
}

/// 为 MemoryEntry 实现 Default（用于测试）
impl Default for MemoryEntry {
    fn default() -> Self {
        Self {
            id: None,
            agent_id: Uuid::new_v4(),
            tick_id: 0,
            event_type: String::new(),
            content: String::new(),
            metadata: Value::Null,
            importance_score: 0.5,
            strength: 0.5,
            embedding: None,
            last_accessed_at: None,
            access_count: 0,
            is_archived: false,
            encoding_valence: None,
            encoding_arousal: None,
            encoding_emotion: None,
            created_at: Utc::now(),
        }
    }
}

/// 嵌入服务状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedderStatus {
    /// 使用本地模型（bge-small-zh-v1.5）
    Local,
    /// 不可用（向量记忆关闭，降级到 FTS5）
    Unavailable,
}

/// 遗忘执行报告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgettingReport {
    /// 检查的记忆数量
    pub checked_count: usize,
    /// 降级到归档的数量
    pub archived_count: usize,
    /// 保留的记忆数量
    pub retained_count: usize,
}

/// 艾宾浩斯配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EbbinghausConfig {
    /// 遗忘率系数
    pub decay_rate: f32,
    /// 最小保留阈值
    pub retention_threshold: f32,
    /// 检索增强系数
    pub retrieval_boost: f32,
}

impl Default for EbbinghausConfig {
    fn default() -> Self {
        Self {
            decay_rate: 0.3,
            retention_threshold: 0.1,
            retrieval_boost: 0.2,
        }
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_entry_creation() {
        let agent_id = Uuid::new_v4();
        let entry = MemoryEntry::new(agent_id, 1, "测试事件".to_string());

        assert_eq!(entry.agent_id, agent_id);
        assert_eq!(entry.tick_id, 1);
        assert_eq!(entry.content, "测试事件");
        assert_eq!(entry.importance_score, 0.5);
        assert_eq!(entry.strength, 0.5);
        assert!(entry.embedding.is_none());
        assert!(entry.last_accessed_at.is_none());
        assert_eq!(entry.access_count, 0);
        assert!(!entry.is_archived);
    }

    #[test]
    fn test_memory_entry_builder() {
        let entry = MemoryEntry::new(Uuid::new_v4(), 1, "测试".to_string())
            .with_event_type("combat".to_string())
            .with_importance(0.8)
            .with_metadata(serde_json::json!({"damage": 10}));

        assert_eq!(entry.event_type, "combat");
        assert_eq!(entry.importance_score, 0.8);
        assert_eq!(entry.strength, 0.8);
        assert_eq!(entry.metadata["damage"], 10);
    }

    #[test]
    fn test_needs_embedding() {
        let high_importance =
            MemoryEntry::new(Uuid::new_v4(), 1, "重要".to_string()).with_importance(0.8);
        assert!(high_importance.needs_embedding());

        let low_importance =
            MemoryEntry::new(Uuid::new_v4(), 1, "普通".to_string()).with_importance(0.5);
        assert!(!low_importance.needs_embedding());

        let with_embedding = MemoryEntry::new(Uuid::new_v4(), 1, "测试".to_string())
            .with_importance(0.8)
            .with_embedding(vec![0.0; 512]);
        assert!(!with_embedding.needs_embedding());
    }

    #[test]
    fn test_ebbinghaus_config_default() {
        let config = EbbinghausConfig::default();
        assert_eq!(config.decay_rate, 0.3);
        assert_eq!(config.retention_threshold, 0.1);
        assert_eq!(config.retrieval_boost, 0.2);
    }
}
