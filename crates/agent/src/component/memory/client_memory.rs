// ============================================================================
// ClientMemory：client_memories 表的行类型与展示转换
// ============================================================================

use chrono::Utc;
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ClientMemory {
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
    /// 重要性评分（0.0-1.0）
    pub importance_score: f32,
    /// 情感评分（-1.0 负面 ~ 1.0 正面）
    pub sentiment_score: f32,
    /// 记忆类型（working, episodic, semantic）
    pub memory_type: String,
    /// 是否已确认（服务端确认的事件）
    pub is_confirmed: bool,
    /// 创建时间
    pub created_at: String,
    /// 更新时间
    pub updated_at: String,
    /// 记忆强度（0.0-1.0，用于遗忘计算）
    pub strength: f32,
    /// 最后访问时间（RFC3339）
    pub last_accessed_at: Option<String>,
    /// 访问次数
    pub access_count: i32,
    /// 是否已归档
    pub is_archived: bool,
    /// 编码时的效价
    pub encoding_valence: Option<f32>,
    /// 编码时的唤醒度
    pub encoding_arousal: Option<f32>,
    /// 编码时的情绪标签
    pub encoding_emotion: Option<String>,
}

impl ClientMemory {
    /// 创建新的记忆
    pub fn new(agent_id: Uuid, tick_id: i64, content: String) -> Self {
        Self {
            id: None,
            agent_id,
            tick_id,
            event_type: "unknown".to_string(),
            content,
            metadata: Value::Null,
            importance_score: 0.5,
            sentiment_score: 0.0,
            memory_type: "episodic".to_string(),
            is_confirmed: true,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
            strength: 0.5,
            last_accessed_at: None,
            access_count: 0,
            is_archived: false,
            encoding_valence: None,
            encoding_arousal: None,
            encoding_emotion: None,
        }
    }

    /// 设置事件类型
    pub fn with_type(mut self, event_type: String) -> Self {
        self.event_type = event_type;
        self
    }

    /// 设置重要性评分
    pub fn with_importance(mut self, importance: f32) -> Self {
        self.importance_score = importance;
        self
    }

    /// 设置元数据
    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = metadata;
        self
    }

    /// 设置记忆类型
    pub fn with_memory_type(mut self, memory_type: String) -> Self {
        self.memory_type = memory_type;
        self
    }
}
