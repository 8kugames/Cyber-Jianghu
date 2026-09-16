//! WebSocket 消息定义
//!
//! 统一的消息格式，确保服务端和客户端兼容。
//!
//! ## 消息流向
//!
//! - [`ServerMessage`] - 服务端 → Agent (注册确认、世界状态、规则更新、错误)
//! - [`ClientMessage`] - Agent → 服务端 (意图上报、心跳、对话)
//!
//! ## 对话系统
//!
//! - [`DialogueMessage`] - Agent 间直接对话 (请求、接受、内容、结束)
//! - [`DialogueSession`] - 服务端维护的对话会话状态

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use uuid::Uuid;

use crate::types::{
    GameRules, GovernanceCode, NarrativeConfig, WorldBuildingRules, WorldEvent, WorldState,
};

// ============================================================================
// 对话消息类型
// ============================================================================

/// 对话消息（Agent 间直接交换）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "message_type", rename_all = "snake_case")]
pub enum DialogueMessage {
    /// 请求建立对话
    Request {
        from_agent_id: Uuid,
        to_agent_id: Uuid,
        opening_remark: String,
    },

    /// 接受对话
    Accept {
        session_id: String,
        from_agent_id: Uuid,
    },

    /// 拒绝对话
    Reject {
        session_id: String,
        from_agent_id: Uuid,
        reason: Option<String>,
    },

    /// 对话内容
    Content {
        session_id: String,
        from_agent_id: Uuid,
        content: String,
    },

    /// 结束对话
    End {
        session_id: String,
        from_agent_id: Uuid,
    },
}

/// 对话会话状态（服务端维护）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DialogueSession {
    pub session_id: String,
    pub agent_a: Uuid,
    pub agent_b: Uuid,
    pub started_at: DateTime<Utc>,
    pub message_count: u32,
}

// ============================================================================
// 服务端消息
// ============================================================================

/// 配置更新类型（Server → Agent 跨进程契约）
///
/// 真闭集：值域由 agent 端 `match &config_type { ... }` 穷尽匹配（7 分支）。
/// 新增类型必须同时修改本枚举、agent dispatch、server 构造点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigType {
    Skills,
    Actions,
    GameRules,
    WorldBuildingRules,
    PromptTemplates,
    PersonaEventRules,
    NarrativeConfig,
}

/// 服务端下发的消息
///
/// # 消息类型
///
/// - `Registered`: Agent 注册成功，包含游戏规则和世界观规则
/// - `WorldState`: 每个 Tick 下发的完整世界状态快照
/// - `ConfigUpdate`: 通用配置更新（统一收拢 game_rules/actions/world_building_rules/skills 下发）
/// - `Pong`: 心跳响应
/// - `Error`: 错误通知
/// - `Dialogue`: 转发 Agent 间对话消息
///
/// # 示例
///
/// ```rust
/// use cyber_jianghu_protocol::{GameRules, ServerMessage};
/// use uuid::Uuid;
///
/// let game_rules = GameRules {
///     tick_duration_secs: 60,
///     available_actions: vec![],
///     initial_items: vec![],
///     survival_actions: vec![],
///     version: "0.0.1".to_string(),
///     last_updated: "2024-01-01T00:00:00Z".to_string(),
///     intent_batch: None,
///     immediate_events: None,
///     rebirth_delay_ticks: 0,
///     rebirth_retry_max_attempts: 3,
///     rebirth_retry_interval_secs: 30,
///     lifespan: None,
///     calendar: None,
///     daily_summary: None,
///     dialogue_context: None,
/// };
///
/// let msg = ServerMessage::Registered {
///     agent_id: Uuid::new_v4(),
///     game_rules,
///     world_building_rules: None,
///     is_alive: true,
///     agent_name: None,
///     narrative_config: None,
///     narrative_config_hash: None,
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum ServerMessage {
    /// 注册成功（包含游戏规则）
    Registered {
        agent_id: Uuid,
        #[serde(flatten)]
        game_rules: GameRules,
        /// 世界观规则（可选，保持向后兼容）
        #[serde(skip_serializing_if = "Option::is_none")]
        world_building_rules: Option<WorldBuildingRules>,
        /// 角色是否存活（由服务器在连接时立即告知）
        is_alive: bool,
        /// 角色名称（可选，首次连接时由服务器填充）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_name: Option<String>,
        /// 叙事化配置（属性描述转换规则）
        #[serde(skip_serializing_if = "Option::is_none")]
        narrative_config: Option<NarrativeConfig>,
        /// 叙事化配置哈希（用于增量跳过）
        #[serde(skip_serializing_if = "Option::is_none")]
        narrative_config_hash: Option<String>,
    },

    /// 世界状态下发
    WorldState {
        #[serde(flatten)]
        data: WorldState,
    },

    /// 通用配置更新（统一收拢所有配置下发消息）
    ///
    /// config_type: [`ConfigType`] 枚举（7 个变体）
    /// update_type: "full" | "incremental"
    /// content: JSON 格式的具体配置内容
    ConfigUpdate {
        /// 配置类型
        config_type: ConfigType,
        /// 更新类型
        update_type: String,
        /// 版本号
        version: String,
        /// 配置内容（JSON 格式）
        content: serde_json::Value,
        /// SHA256 hex of canonical JSON（用于 skip-optimization）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_hash: Option<String>,
        /// 增量更新的项目 ID 列表（增量时有效）
        #[serde(default)]
        updated_items: Vec<String>,
        /// 被删除的项目 ID 列表（增量时有效）
        #[serde(default)]
        removed_items: Vec<String>,
    },

    /// 心跳响应
    Pong { timestamp: i64 },

    /// 错误消息
    Error {
        /// 机器可读错误码（如 "tick_mismatch", "agent_dead"）
        /// 详见 `crate::ERROR_CODE_*` 常量
        #[serde(default, skip_serializing_if = "String::is_empty")]
        code: String,
        /// 人类可读错误描述
        message: String,
        /// tick 不匹配时的当前 tick_id（仅 tick_mismatch 有值）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        current_tick_id: Option<i64>,
    },

    /// 对话消息（转发）
    Dialogue {
        #[serde(flatten)]
        message: DialogueMessage,
    },

    /// Agent 死亡通知
    ///
    /// 当 Agent 因任何原因死亡时，Server 立即推送此消息。
    /// Agent 收到后透传给 OpenClaw，触发重生流程。
    AgentDied {
        /// 死亡的 Agent ID
        agent_id: Uuid,
        /// 死亡原因代码（来自配置：satiation, hydration, environmental, combat, etc.）
        cause: String,
        /// 死亡描述（来自配置，叙事化文本）
        description: String,
        /// 死亡位置（node_id）
        location: String,
        /// 当前 tick
        tick_id: i64,
        /// 死亡时间戳（Unix timestamp, 毫秒）
        died_at: i64,
        /// 重生等待时间（tick 数，0 = 立即，-1 = 不可重生）
        rebirth_delay_ticks: i32,
        /// 死亡上下文（属性快照/存活时间/最后行为，供同地 Agent 学习）
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
    },

    /// 立即事件（speak 等需要立即广播的事件）
    ///
    /// 与 WorldState 不同，ImmediateEvent 只包含单个事件，用于：
    /// - speak 广播：同场景所有在线 Agent 立即收到
    /// - 其他需要实时通知的事件
    ImmediateEvent {
        /// 事件唯一 ID（用于即时意图追踪）
        event_id: Uuid,
        /// 事件内容
        event: WorldEvent,
    },

    /// 实时意图执行结果（实时模式下，IntentWorker 处理后立即返回）
    ExecutionResult {
        /// 处理时的 tick_id
        tick_id: i64,
        /// 原始 Intent ID
        intent_id: Uuid,
        /// 是否成功
        success: bool,
        /// 失败原因（success=false 时有值）
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// 状态变更摘要（如 "吃了馒头, 饱食度+20"）
        #[serde(skip_serializing_if = "Option::is_none")]
        state_change_summary: Option<String>,
        /// Server 端映射后的治理分类码
        #[serde(skip_serializing_if = "Option::is_none")]
        governance_code: Option<GovernanceCode>,
    },
}

// ============================================================================
// 客户端消息
// ============================================================================

/// Agent 上报的消息
///
/// # 消息类型
///
/// - `Intent`: Agent 意图上报 (每 Tick 一次，包含动作类型和参数)
/// - `Ping`: 心跳请求
/// - `Dialogue`: Agent 间对话消息 (通过服务端转发)
///
/// # 意图上报示例
///
/// ```rust
/// use cyber_jianghu_protocol::ClientMessage;
/// use serde_json::json;
///
/// let msg = ClientMessage::Intent {
///     intent_id: None,
///     tick_id: 1,
///     agent_id: None,
///     thought_log: Some("思考过程".to_string()),
///     action_type: "说话".to_string(),
///     action_data: Some(json!({"content": "你好"})),
///     priority: 5,
///     subsequent_intents: vec![],
///     soul_cycle_metadata: None,
///     chaos_marker: None,
///     dream_marker: None,
/// };
/// ```
///
/// 与服务器端格式保持一致，使用扁平化字段
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// 意图上报
    Intent {
        /// Intent 唯一 ID（可选，如果未提供则服务端自动生成）
        #[serde(skip_serializing_if = "Option::is_none")]
        intent_id: Option<Uuid>,
        /// Tick 编号
        tick_id: i64,
        /// Agent ID（可选，不提供则使用连接关联的 agent）
        /// 用于支持同一设备上的多角色切换
        #[serde(skip_serializing_if = "Option::is_none")]
        agent_id: Option<Uuid>,
        /// 思考日志
        #[serde(skip_serializing_if = "Option::is_none")]
        thought_log: Option<String>,
        /// 动作类型
        action_type: String,
        /// 动作参数
        #[serde(skip_serializing_if = "Option::is_none")]
        action_data: Option<serde_json::Value>,
        /// 优先级
        priority: i32,
        /// Pipeline 后续 Intent（multi-Intent 支持）
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        subsequent_intents: Vec<crate::types::Intent>,
        /// 三魂循环元数据（随 intent 一次性提交，消除独立 SoulCycleReport 的丢失风险）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        soul_cycle_metadata: Option<SoulCycleMetadata>,
        /// 混沌行为标记（随 intent 提交，server 据此渲染"陷入混乱"徽章）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chaos_marker: Option<crate::types::ChaosMarker>,
        /// 托梦影响标记（随 intent 提交，server 据此渲染"受托梦影响"徽章）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dream_marker: Option<crate::types::DreamMarker>,
    },

    /// 对话消息
    Dialogue {
        #[serde(flatten)]
        message: DialogueMessage,
    },

    /// 三魂循环元数据上报（agent → server）
    ///
    /// 在 intent 发送后立即发送此消息，server 将其关联到同一 tick 的 agent_action_logs。
    /// 作用：使 server-web 能看到与 agent-web 完全相同的三魂详情。
    SoulCycleReport {
        /// Tick 编号
        tick_id: i64,
        /// Agent ID（可选）
        #[serde(skip_serializing_if = "Option::is_none")]
        agent_id: Option<Uuid>,
        /// Pipeline 序列号（对应同一 tick 内的多个 intent）
        pipe_seq: i32,
        /// 三魂循环完整元数据
        metadata: SoulCycleMetadata,
    },

    /// 每日 LLM 日志摘要上报（agent → server）
    ///
    /// 游戏日结束时由 SessionTriageEngine 生成，提交给 Server 存档。
    /// Server 接收时注入 created_at 时间戳（服务器权威时间，非客户端）。
    DailySummary {
        /// 游戏日编号
        game_day: i64,
        /// 格式化摘要内容（由 session_triage.rs 的 produce_daily_summary 生成）
        summary: String,
    },

    /// 关系图谱全量快照上报（agent → server）
    ///
    /// 游戏日结束时随 DailySummary 一起发送，server 全量覆盖（DELETE+INSERT），
    /// 天然幂等。携带 agent 端完整关系列表（对齐 protocol::types::RelationshipMemory 契约）。
    RelationshipSnapshot {
        /// 关系持有者
        agent_id: uuid::Uuid,
        /// 所属游戏日（用于幂等追踪）
        game_day: i64,
        /// 完整关系列表（全量覆盖）
        relationships: Vec<crate::types::RelationshipMemory>,
    },

    /// 训练 Trace 上报（agent → server）
    ///
    /// agent 端的结构化 LLM 调用 trace（已脱敏），批量回传 server 汇聚。
    /// server 落盘后与 reward 同目录树，训练导出时按 (agent_id, tick_id) join。
    TraceReport {
        /// 批量 trace 条目（已脱敏）
        #[serde(default)]
        traces: Vec<TraceEntry>,
    },
}

/// 训练 Trace 条目（agent 端 LlmTrace 的协议层对应，已脱敏）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEntry {
    pub trace_id: String,
    pub agent_id: Uuid,
    pub character_name: String,
    pub tick_id: i64,
    pub soul_stage: String,
    pub attempt: i32,
    pub provider: String,
    pub model: String,
    /// 角色设定（agent 特有部分；静态 system 模板由项目配置复用）
    #[serde(default)]
    pub persona_name: String,
    #[serde(default)]
    pub persona_description: String,
    pub user_prompt: String,
    pub response: String,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub ok: bool,
    /// trace 产生时间（Unix 毫秒，agent 端真实时间，非 server 接收时间）
    #[serde(default)]
    pub wall_clock: Option<i64>,
}

/// 三魂循环元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoulCycleMetadata {
    /// 游戏内时间（用于经历日志显示）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_time: Option<String>,
    /// 三魂循环记录
    pub cycles: Vec<SoulCycleAttempt>,
    /// 即时通道意图记录
    pub immediate_intents: Vec<ImmediateIntentReport>,
}

/// 单次三魂尝试
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoulCycleAttempt {
    pub attempt: i32,
    /// 人魂输出
    pub renhun: RenhunReport,
    /// 天魂三层审查结果
    pub tianhun: TianhunReport,
    /// 最终 Intent
    pub final_intent: Option<FinalIntentReport>,
    /// 该次尝试使用的 LLM 模型 ID（用于经历日志展示）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
}

/// 地魂 tool calling 日志条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EarthToolCall {
    /// 工具名称: query_world, get_action_detail, list_skills, skill_view 等
    pub name: String,
    /// 调用参数（JSON 序列化后的字符串）
    pub arguments: String,
    /// 结果摘要（截断后的原始结果，非 budget 处理后）
    pub result_summary: String,
    /// 执行是否成功
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenhunReport {
    pub narrative: Option<String>,
    pub thought_log: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub earth_tool_calls: Option<Vec<EarthToolCall>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TianhunReport {
    pub result: Option<String>,
    pub layers: Vec<LayerReport>,
    pub reason: Option<String>,
    /// 多意图 pipeline 逐意图审查结果（按送审顺序）。
    /// layers 字段仅保留末意图结果（旧展示兼容）；新数据优先进本字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_intent_layers: Option<Vec<IntentLayersReport>>,
}

/// 单个意图的天魂审查结果（多意图 pipeline 聚合条目）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentLayersReport {
    /// 意图标签（动作名，可能带后缀标记审查路径：
    /// 「(自纠)」自纠通过、「(自纠·驳回)」自纠仍驳回、「(自纠·LLM失败)」自纠
    /// LLM 调用失败、「(chaos)」chaos 替补意图（不经天魂审查，layers 为空））
    pub intent: String,
    pub layers: Vec<LayerReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerReport {
    pub layer: String,
    pub passed: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineAction {
    pub action_type: String,
    pub action_data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalIntentReport {
    pub intent_id: Option<String>,
    pub action_type: Option<String>,
    pub action_data: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline_actions: Option<Vec<PipelineAction>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chaos_marker: Option<crate::types::ChaosMarker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dream_marker: Option<crate::types::DreamMarker>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImmediateIntentReport {
    pub intent_id: String,
    pub route_type: String,
    pub action_type: String,
    pub action_data: Option<serde_json::Value>,
    /// 说话者名称（用于 server-web 渲染说话对象）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_agent_name: Option<String>,
    pub speech_content: Option<String>,
    pub send_status: String,
    pub send_error: Option<String>,
}

// ============================================================================
// 消息解析辅助
// ============================================================================

impl ClientMessage {
    /// 从 JSON 字符串解析
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// 转换为 JSON 字符串
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// 从 Intent 创建 ClientMessage
    pub fn from_intent(intent: crate::types::Intent) -> Self {
        ClientMessage::Intent {
            intent_id: Some(intent.intent_id),
            tick_id: intent.tick_id,
            agent_id: Some(intent.agent_id),
            thought_log: intent.thought_log,
            action_type: intent.action_type.to_string(),
            action_data: intent.action_data,
            priority: intent.priority,
            subsequent_intents: intent.subsequent_intents,
            soul_cycle_metadata: None,
            chaos_marker: None,
            dream_marker: None,
        }
    }

    /// 从 Intent 创建 ClientMessage（携带三魂元数据 + 标记）
    ///
    /// 三魂审查在 intent 提交前完成，metadata 此时已就绪。
    /// 随 intent 一次性提交，消除独立 SoulCycleReport 消息的丢失风险。
    pub fn from_intent_with_extras(
        intent: crate::types::Intent,
        soul_cycle_metadata: Option<SoulCycleMetadata>,
    ) -> Self {
        let chaos_marker = intent.chaos_marker.clone();
        let dream_marker = intent.dream_marker.clone();
        ClientMessage::Intent {
            intent_id: Some(intent.intent_id),
            tick_id: intent.tick_id,
            agent_id: Some(intent.agent_id),
            thought_log: intent.thought_log,
            action_type: intent.action_type.to_string(),
            action_data: intent.action_data,
            priority: intent.priority,
            subsequent_intents: intent.subsequent_intents,
            soul_cycle_metadata,
            chaos_marker,
            dream_marker,
        }
    }

    /// 从 DialogueMessage 创建 ClientMessage
    pub fn from_dialogue(message: DialogueMessage) -> Self {
        ClientMessage::Dialogue { message }
    }
}

impl ServerMessage {
    /// 从 JSON 字符串解析
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// 转换为 JSON 字符串
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// 全量配置下发：content 与 content_hash 由同一 payload 序列化产生，保证两者口径一致
    pub fn config_update_full<T: serde::Serialize>(
        config_type: ConfigType,
        version: impl Into<String>,
        payload: &T,
    ) -> Self {
        Self::config_update_full_value(
            config_type,
            version,
            serde_json::to_value(payload).unwrap_or_default(),
            payload_hash(payload),
        )
    }

    /// 全量配置下发（内容已就绪；content_hash 可为 None）
    pub fn config_update_full_value(
        config_type: ConfigType,
        version: impl Into<String>,
        content: serde_json::Value,
        content_hash: Option<String>,
    ) -> Self {
        Self::ConfigUpdate {
            config_type,
            update_type: "full".to_string(),
            version: version.into(),
            content,
            content_hash,
            updated_items: vec![],
            removed_items: vec![],
        }
    }
}

/// 序列化 payload 并计算 SHA256 hex（用于 ConfigUpdate 的 skip-optimization）
pub fn payload_hash<T: serde::Serialize>(payload: &T) -> Option<String> {
    serde_json::to_vec(payload)
        .ok()
        .map(|bytes| format!("{:x}", sha2::Sha256::digest(&bytes)))
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
#[path = "messages_tests.rs"]
mod tests;
