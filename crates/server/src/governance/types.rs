use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use cyber_jianghu_protocol::types::governance::{GovernanceTopic, ProposedActionIR};

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

/// 投票选择
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoteChoice {
    Approve,
    Reject,
    Abstain,
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
}

/// 提案证据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalEvidence {
    pub agent_id: uuid::Uuid,
    pub tick_id: i64,
    pub proposed_action_type: String,
    pub ir: ProposedActionIR,
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
}

fn default_confidence_threshold() -> f64 {
    0.6
}

fn default_fallback_topic() -> String {
    "evolution".to_string()
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
}
