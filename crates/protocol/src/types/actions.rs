//! 动作和意图相关类型
//!
//! 数据驱动设计：ActionType 是字符串，不限制具体值
//! 可用动作类型由 WorldState.available_actions 动态提供

use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::Deref;
use std::str::FromStr;
use uuid::Uuid;

/// 动作类型 - 完全数据驱动
///
/// 不再使用枚举，而是使用字符串包装类型。
/// 具体可用动作从 WorldState.available_actions 获取。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(transparent)]
pub struct ActionType(String);

impl ActionType {
    /// 创建新的动作类型
    pub fn new(action: impl Into<String>) -> Self {
        Self(action.into())
    }

    /// 获取动作类型字符串
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 常用动作类型常量（中文）
    /// idel/休整 作为默认值；speak/说话、attack/攻击 在 rule_engine 和集成测试中使用。
    /// 其余动作类型由 actions.yaml 数据驱动，不在此处硬编码。
    pub const IDLE: &'static str = "休整";
    pub const SPEAK: &'static str = "说话";
    pub const ATTACK: &'static str = "攻击";
}

impl Default for ActionType {
    fn default() -> Self {
        Self(Self::IDLE.to_string())
    }
}

impl fmt::Display for ActionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for ActionType {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ActionType {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<ActionType> for String {
    fn from(action: ActionType) -> Self {
        action.0
    }
}

impl Deref for ActionType {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for ActionType {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for ActionType {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_string()))
    }
}

// ============================================================================
// 混沌标记
// ============================================================================

/// 混沌行为来源标记，用于前端结构化展示"陷入混乱"徽章
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "detail")]
pub enum ChaosMarker {
    /// 低理智触发（sanity < activation_threshold）
    Sanity { sanity: i32 },
    /// LLM 配额耗尽触发（连续认知失败 >= llm_chaos_threshold）
    LlmQuotaExhausted { consecutive_failures: usize },
}

// ============================================================================
// 托梦标记
// ============================================================================

/// 托梦影响标记，用于前端渲染"受托梦影响"徽章及 DB 追溯
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamMarker {
    /// 托梦内容摘要（前 50 字）
    pub thought: String,
}

// ============================================================================
// 意图
// ============================================================================

/// Agent 上报的意图
///
/// 每个 Tick，Agent 通过 WebSocket 上报意图，包含要执行的动作
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intent {
    /// Intent 唯一 ID，用于全链路追踪
    #[serde(default = "uuid::Uuid::new_v4")]
    pub intent_id: Uuid,

    /// Agent ID
    pub agent_id: Uuid,

    /// Tick 编号
    pub tick_id: i64,

    /// 思考日志（Agent 的内心独白）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_log: Option<String>,

    /// 动作类型（数据驱动，任意字符串）
    pub action_type: ActionType,

    /// 动作参数（JSON 格式）
    ///
    /// 不同动作类型的参数由服务端配置定义
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_data: Option<serde_json::Value>,

    /// 优先级（1-10，1 最高）
    #[serde(default = "default_priority")]
    pub priority: i32,

    /// 反思之魂的审查意见（result + reason）
    ///
    /// 审查通过时包含 reason，审查拒绝时也包含 reason
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflector_thought: Option<String>,

    /// 混沌行为标记（前端据此渲染"陷入混乱"徽章）
    ///
    /// None = 正常决策；Some = 混沌降级行为
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chaos_marker: Option<ChaosMarker>,

    /// 托梦影响标记（前端据此渲染"受托梦影响"徽章）
    ///
    /// None = 未受托梦影响；Some = 本 tick 有活跃托梦
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dream_marker: Option<DreamMarker>,

    /// 是否已经广播（用于 speak 动作的幂等性）
    ///
    /// speak 动作在 handle_intent 时立即广播给同 Location 的 Agent，
    /// 结算时通过此字段跳过重复广播
    #[serde(default)]
    pub already_broadcast: bool,

    /// 关联的 Dialogue Session ID（用于 whisper 动作）
    ///
    /// whisper 动作在 handle_intent 时立即建立 Dialogue Session，
    /// 用于关单时强制结束 Session
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,

    /// 原子意图队列（multi-Intent 支持）
    ///
    /// `subsequent_intents` 是一个**原子意图**的队列，绝不是复合动作。
    /// 主 Intent 独立执行成功后，Server 按顺序将后续 Intent 当作独立的原子意图逐个执行。
    /// 任一失败则仅回滚该失败的原子意图，并跳过后续所有排队的 Intent（已成功的保持成功）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subsequent_intents: Vec<Intent>,
}

fn default_priority() -> i32 {
    5
}

impl Intent {
    /// 创建通用意图（数据驱动）
    pub fn new(
        agent_id: Uuid,
        tick_id: i64,
        action_type: impl Into<ActionType>,
        action_data: Option<serde_json::Value>,
    ) -> Self {
        Self {
            intent_id: Uuid::new_v4(),
            agent_id,
            tick_id,
            thought_log: None,
            action_type: action_type.into(),
            action_data,
            priority: 5,
            reflector_thought: None,
            chaos_marker: None,
            dream_marker: None,
            already_broadcast: false,
            session_id: None,
            subsequent_intents: vec![],
        }
    }

    /// 创建带 intent_id 的通用意图（数据驱动）
    pub fn new_with_id(
        intent_id: Uuid,
        agent_id: Uuid,
        tick_id: i64,
        action_type: impl Into<ActionType>,
        action_data: Option<serde_json::Value>,
    ) -> Self {
        Self {
            intent_id,
            agent_id,
            tick_id,
            thought_log: None,
            action_type: action_type.into(),
            action_data,
            priority: 5,
            reflector_thought: None,
            chaos_marker: None,
            dream_marker: None,
            already_broadcast: false,
            session_id: None,
            subsequent_intents: vec![],
        }
    }

    /// 设置思考日志
    pub fn with_thought(mut self, thought: String) -> Self {
        self.thought_log = Some(thought);
        self
    }

    /// 设置反思之魂审查意见
    pub fn with_reflector_thought(mut self, thought: String) -> Self {
        self.reflector_thought = Some(thought);
        self
    }

    /// 设置混沌标记
    pub fn with_chaos_marker(mut self, marker: ChaosMarker) -> Self {
        self.chaos_marker = Some(marker);
        self
    }

    /// 设置托梦标记
    pub fn with_dream_marker(mut self, marker: DreamMarker) -> Self {
        self.dream_marker = Some(marker);
        self
    }

    /// 将 Intent 及其 subsequent_intents 展开为 Vec
    pub fn as_pipeline(&self) -> Vec<Intent> {
        let mut intents = vec![self.clone()];
        intents.extend(self.subsequent_intents.clone());
        intents
    }
}

// ============================================================================
// Pipeline 执行结果
// ============================================================================

/// 单个 Intent 执行状态
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentExecutionStatus {
    /// 执行成功
    Success,
    /// 部分成功（如数量不足时部分执行）
    PartialSuccess,
    /// 执行失败
    Failed,
    /// 跳过（前置 Intent 失败）
    Skipped,
}

/// 单个 Intent 执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentExecutionResult {
    /// Intent ID
    pub intent_id: Uuid,
    /// 执行状态
    pub status: IntentExecutionStatus,
    /// 实际执行数量（用于 partial success）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executed_quantity: Option<i32>,
    /// 失败原因
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_reason: Option<String>,
}

/// Pipeline 执行汇总（广播用，不含私有数据）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionSummary {
    /// 总 Intent 数
    pub total: usize,
    /// 成功数
    pub succeeded: usize,
    /// 部分成功数
    pub partial: usize,
    /// 失败数
    pub failed: usize,
    /// 跳过数
    pub skipped: usize,
}

impl ExecutionSummary {
    /// 从执行结果列表生成汇总
    pub fn from_results(results: &[IntentExecutionResult]) -> Self {
        let mut summary = Self {
            total: results.len(),
            succeeded: 0,
            partial: 0,
            failed: 0,
            skipped: 0,
        };
        for r in results {
            match r.status {
                IntentExecutionStatus::Success => summary.succeeded += 1,
                IntentExecutionStatus::PartialSuccess => summary.partial += 1,
                IntentExecutionStatus::Failed => summary.failed += 1,
                IntentExecutionStatus::Skipped => summary.skipped += 1,
            }
        }
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_type_new() {
        let action = ActionType::new("custom_action");
        assert_eq!(action.as_str(), "custom_action");
    }

    #[test]
    fn test_action_type_serde() {
        let action = ActionType::new("休整");
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(json, "\"休整\"");

        let parsed: ActionType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.as_str(), "休整");
    }

    #[test]
    fn test_action_type_custom() {
        let action = ActionType::new("攻击");
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(json, "\"攻击\"");

        let parsed: ActionType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.as_str(), "攻击");
    }

    #[test]
    fn test_intent_new() {
        let agent_id = Uuid::new_v4();
        let intent = Intent::new(
            agent_id,
            1,
            "custom_action",
            Some(serde_json::json!({ "param": "value" })),
        );
        assert_eq!(intent.action_type.as_str(), "custom_action");
        assert_eq!(intent.tick_id, 1);
    }

    #[test]
    fn test_intent_idle() {
        let agent_id = Uuid::new_v4();
        let intent = Intent::new(agent_id, 1, "休整", None);
        assert_eq!(intent.action_type.as_str(), "休整");
        assert_eq!(intent.tick_id, 1);
    }

    #[test]
    fn test_intent_speak() {
        let agent_id = Uuid::new_v4();
        let intent = Intent::new(
            agent_id,
            2,
            "说话",
            Some(serde_json::json!({"content": "Hello"})),
        );
        assert_eq!(intent.action_type.as_str(), "说话");
        assert!(intent.action_data.is_some());
    }

    #[test]
    fn test_intent_with_thought() {
        let agent_id = Uuid::new_v4();
        let intent = Intent::new(agent_id, 1, "休整", None).with_thought("Thinking...".to_string());
        assert_eq!(intent.thought_log, Some("Thinking...".to_string()));
    }
}
