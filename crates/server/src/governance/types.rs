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
///
/// 注：管道设计不允许弃权。LLM 必须输出 approve 或 reject；
/// LLM 调用超时/失败时由系统强制注入 Reject（reject_reason="other"）。
/// 保留 Abstain 仅为内部 fallback 标识，不应进入决议统计。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoteChoice {
    Approve,
    Reject,
    #[allow(dead_code)]
    Abstain,
}

/// 三皇共审管道阶段
///
/// 持久化于 `action_evolution_proposal_groups.stage` 列。
/// 轮询任务（main.rs）按 stage 路由到对应处理函数，每轮只推进一个阶段，
/// 多轮跨周期完成完整管道。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStage {
    /// 阶段 1：等待伏羲初审
    ///
    /// group 创建后初始状态。close_stale_groups 仅作用于此阶段超时的 group
    /// —— 长时间停留意味着数据陈旧或 LLM 持续失败，应强制关闭。
    #[default]
    AwaitingFuxiInitial,
    /// 阶段 2：等待神农 + 轩辕并行审议
    ///
    /// 伏羲初审已 approve。神农/轩辕将通过 tokio::join! 并行审议。
    AwaitingPeer,
    /// 阶段 3：等待伏羲终审调整 + 写入
    ///
    /// 同辈审议已达成 approve_threshold。伏羲终审时注入同辈反馈，
    /// 可能调整 inferred_action_config 以满足附条件批准的合理顾虑。
    AwaitingFuxiFinal,
    /// 管道完成（已 approved / rejected / closed / escalated_admin / error）
    ///
    /// done 状态下同名 action 重新提议时，upsert_proposal_group 会重置
    /// stage 为 awaiting_fuxi_initial 重新启动管道。
    Done,
}

impl ProposalStage {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AwaitingFuxiInitial => "awaiting_fuxi_initial",
            Self::AwaitingPeer => "awaiting_peer",
            Self::AwaitingFuxiFinal => "awaiting_fuxi_final",
            Self::Done => "done",
        }
    }

    pub fn from_db_str(s: &str) -> Self {
        match s {
            "awaiting_peer" => Self::AwaitingPeer,
            "awaiting_fuxi_final" => Self::AwaitingFuxiFinal,
            "done" => Self::Done,
            _ => Self::AwaitingFuxiInitial,
        }
    }
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

/// 审议结果（单个 soul 对单个 proposal 的裁决）
///
/// 由 `GovernanceLlmClient::review_with_llm` 返回，或由 engine 在硬规则命中时构造。
/// 持久化时拆解到 `soul_review_votes` 表（不含 inferred_action_config 与 reject_reason）。
#[derive(Debug, Clone)]
pub struct ReviewVerdict {
    pub soul: String,
    pub vote: VoteChoice,
    pub rationale: String,
    pub evidence_refs: Vec<String>,
    /// reject 时细分原因（approve 时为 None）
    ///
    /// 三皇 prompt 必须输出对应的字符串值：non_atomic / governance_value / other
    pub reject_reason: Option<RejectReason>,
    /// approve 时附带 LLM 推断的 actions.yaml 字段
    ///
    /// 仅在伏羲初审/终审 approve 时填充。神农/轩辕的 verdict 此字段为 None
    /// （写入 actions.yaml 的字段以伏羲终审推断为准）。
    pub inferred_action_config: Option<InferredActionConfig>,
}

impl ReviewVerdict {
    /// 构造 abstain fallback（用于 soul 未注册等内部场景）
    ///
    /// 注：管道不允许 LLM 输出 abstain（LLM 超时/失败由 llm_review::system_reject 强制 reject）。
    /// 此函数仅用于非 LLM 调用路径的内部 fallback。
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
    /// 三皇共审 approve 阈值（含伏羲初审票数）
    /// 默认 2 = 伏羲初审 + 至少一票同辈批准
    pub approve_threshold: u8,
    pub poll_interval_secs: u64,
    /// proposal_group 生命周期超时（秒），超过此值未闭环的 group 强制关闭
    /// 仅作用于 stage='awaiting_fuxi_initial' 的 group（管道开始就卡住）
    #[serde(default = "default_group_stale_secs")]
    pub group_stale_secs: u64,
}

fn default_group_stale_secs() -> u64 {
    1800
}
