use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use cyber_jianghu_protocol::types::governance::{
    AtomicKind, GovernanceTopic, ProtocolKind, TargetArity,
};

/// 审议角色
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewRole {
    Primary,
    CoReviewer,
}

/// 提案组状态机
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    PendingReview,
    UnderReview,
    Approved,
    Rejected,
    EscalatedAdmin,
    Converged,
    ClosedApproved,
    ClosedRejected,
    Error,
}

impl std::fmt::Display for ProposalStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = serde_json::to_value(self)
            .map(|v| v.as_str().unwrap_or("unknown").to_string())
            .unwrap_or_else(|_| format!("{:?}", self).to_lowercase());
        write!(f, "{}", s)
    }
}

impl ProposalStatus {
    pub fn from_db_str(s: &str) -> Self {
        serde_json::from_value(serde_json::Value::String(s.to_string())).unwrap_or(Self::Error)
    }
}

/// 投票选择
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoteChoice {
    Approve,
    Reject,
    Abstain,
}

/// Reject 细分原因（伏羲 LLM 输出）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    /// 非原子性：动作涉及多执行者/多阶段/跨 tick
    NonAtomic,
    /// 不符合演化方向/世界观
    GovernanceValue,
    /// 其他原因（在 rationale 中说明）
    Other,
}

/// 伏羲 LLM 推断的动作配置（approve 时附带，写入 actions.yaml）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferredActionConfig {
    pub atomic_kind: AtomicKind,
    pub actor_arity: u8,
    pub target_arity: TargetArity,
    pub tick_span: u8,
    pub phase_count: u8,
    pub protocol_kind: ProtocolKind,
    pub effect_refs: Vec<String>,
    pub requirement_refs: Vec<String>,
}

/// 分类结果
#[derive(Debug, Clone)]
pub struct ClassificationResult {
    pub topics: Vec<GovernanceTopic>,
    pub confidence: HashMap<GovernanceTopic, f64>,
    pub fallback_used: bool,
}

/// 路由计划
#[derive(Debug, Clone)]
pub struct RoutePlan {
    pub primary_soul: Option<String>,
    pub co_reviewers: Vec<String>,
    pub escalate: bool,
}

/// 审议结果
#[derive(Debug, Clone)]
pub struct ReviewVerdict {
    pub soul: String,
    pub vote: VoteChoice,
    pub rationale: String,
    pub evidence_refs: Vec<String>,
    /// reject 时细分原因（approve/abstain 时为 None）
    pub reject_reason: Option<RejectReason>,
    /// approve 时附带 LLM 推断的 actions.yaml 字段
    pub inferred_action_config: Option<InferredActionConfig>,
}

impl ReviewVerdict {
    /// 构造 abstain fallback（用于 LLM 未启用、调用失败等场景）
    pub fn abstain(soul: impl Into<String>, rationale: impl Into<String>) -> Self {
        Self {
            soul: soul.into(),
            vote: VoteChoice::Abstain,
            rationale: rationale.into(),
            evidence_refs: vec![],
            reject_reason: None,
            inferred_action_config: None,
        }
    }
}

/// 提案证据
///
/// 提案触发条件是 agent 端 UnknownAction，agent 无可信执行特征（IR）数据源。
/// 携带 agent 的 intent 上下文（action_data），由伏羲 LLM 审议时推断原子性
/// 与执行特征。actions.yaml 是运行时真相，DB 中不再存 IR 字段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalEvidence {
    pub agent_id: uuid::Uuid,
    pub tick_id: i64,
    pub proposed_action_type: String,
    /// Agent intent 上下文（target_agent_id / item_id / quantity 等完整参数）
    pub action_data: serde_json::Value,
    pub governance_topics: Vec<GovernanceTopic>,
    pub topic_confidence: HashMap<GovernanceTopic, f64>,
    pub rationale: String,
}

/// Soul 配置
#[derive(Debug, Clone, Deserialize)]
pub struct SoulConfig {
    pub display_name: String,
    pub governance_role: String,
    pub review_policy: ReviewPolicy,
    pub source_bindings: HashMap<String, serde_json::Value>,
    pub system_prompt_template: String,
}

/// 审议策略
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ReviewPolicy {
    #[serde(default)]
    pub hard_approve_if: Vec<PolicyRule>,
    #[serde(default)]
    pub hard_reject_if: Vec<PolicyRule>,
    #[serde(default)]
    pub soft_concern_if: Vec<PolicyRule>,
}

/// 策略规则
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PolicyRule {
    Metric {
        metric: String,
        operator: String,
        threshold: f64,
        #[serde(default)]
        reason: String,
    },
    EffectRef {
        effect_ref_matches: Vec<String>,
        #[serde(default)]
        reason: String,
        #[serde(default)]
        requires: Vec<String>,
    },
    EffectGroup {
        all_effects_in: Vec<String>,
    },
}

/// Cap entry in manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityEntry {
    pub capability_id: String,
    pub kind: String,
    pub semantic_scope: String,
}

/// 演化治理配置
#[derive(Debug, Clone, Deserialize)]
pub struct ActionEvolutionConfig {
    pub capability_policy: CapabilityPolicy,
    pub topic_classifier: TopicClassifierConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CapabilityPolicy {
    pub allowed_capability_groups: Vec<String>,
    pub denied_capability_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TopicClassifierConfig {
    pub rules: Vec<TopicClassifierRule>,
    #[serde(default = "default_confidence_threshold")]
    pub confidence_threshold: f64,
    #[serde(default = "default_fallback_topic")]
    pub default_fallback_topic: String,
    #[serde(default = "default_fallback_confidence")]
    pub fallback_confidence: f64,
}

fn default_confidence_threshold() -> f64 {
    0.6
}

fn default_fallback_topic() -> String {
    "evolution".to_string()
}

fn default_fallback_confidence() -> f64 {
    0.5
}

#[derive(Debug, Clone, Deserialize)]
pub struct TopicClassifierRule {
    #[serde(rename = "match")]
    pub matcher: TopicClassifierMatch,
    pub topics: Vec<GovernanceTopic>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TopicClassifierMatch {
    #[serde(default)]
    pub effect_refs_prefix: Vec<String>,
    #[serde(default)]
    pub effect_refs_any: Vec<String>,
}

/// Souls 配置（顶层）
#[derive(Debug, Clone, Deserialize)]
pub struct SoulsConfig {
    pub souls: HashMap<String, SoulConfig>,
    pub topic_to_soul: HashMap<String, String>,
    pub topic_priority: HashMap<String, u8>,
    pub classifier: SoulsClassifierConfig,
    pub review: SoulsReviewConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SoulsClassifierConfig {
    pub confidence_threshold: f64,
    pub default_fallback_topic: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SoulsReviewConfig {
    pub timeout_secs: u64,
    pub dissent_log_threshold: u32,
    pub approve_threshold: u8,
    pub reject_threshold: u8,
    pub poll_interval_secs: u64,
    /// proposal_group 生命周期超时（秒），超过此值未闭环的 group 强制关闭
    /// 与 timeout_secs（LLM 调用超时）语义独立，避免混淆
    #[serde(default = "default_group_stale_secs")]
    pub group_stale_secs: u64,
}

fn default_group_stale_secs() -> u64 {
    1800
}
