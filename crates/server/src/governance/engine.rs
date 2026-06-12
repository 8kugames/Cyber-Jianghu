use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::{error, info, warn};

use cyber_jianghu_protocol::GovernanceTopic;

use super::llm_review::{GovernanceLlmClient, build_review_message, build_soul_prompt};
use super::manifest::CapabilityManifest;
use super::proposal_store::{PendingGroup, ProposalStore};
use super::types::{
    PolicyRule, ProposalEvidence, ProposalStatus, ReviewRole, ReviewVerdict, SoulsConfig,
    VoteChoice,
};

// ---------------------------------------------------------------------------
// SourceProvider trait
// ---------------------------------------------------------------------------

/// 数据源提供者 — 为 Soul 审议提供外部指标数据
#[async_trait::async_trait]
pub trait SourceProvider: Send + Sync {
    /// 获取指定 metric 的当前值
    async fn get_metric(&self, metric: &str) -> Result<f64>;

    /// 检查 effect_ref 是否匹配
    fn match_effect_ref(&self, effect_refs: &[String], pattern: &str) -> bool;
}

// ---------------------------------------------------------------------------
// NullSourceProvider
// ---------------------------------------------------------------------------

/// 空实现 — 所有 metric 返回 0.0，effect_ref 精确匹配
pub struct NullSourceProvider;

#[async_trait::async_trait]
impl SourceProvider for NullSourceProvider {
    async fn get_metric(&self, _metric: &str) -> Result<f64> {
        Ok(0.0)
    }

    fn match_effect_ref(&self, effect_refs: &[String], pattern: &str) -> bool {
        effect_refs.iter().any(|e| e == pattern)
    }
}

// ---------------------------------------------------------------------------
// SoulReviewEngine
// ---------------------------------------------------------------------------

pub struct SoulReviewEngine {
    config: SoulsConfig,
    sources: HashMap<String, Box<dyn SourceProvider>>,
    llm_client: Arc<GovernanceLlmClient>,
    capability_manifest: CapabilityManifest,
}

impl SoulReviewEngine {
    pub fn load(config_dir: &Path) -> Result<Self> {
        let yaml_path = config_dir.join("souls.yaml");
        let yaml_content = std::fs::read_to_string(&yaml_path).context("读取 souls.yaml 失败")?;
        let outer: serde_json::Value =
            serde_yaml::from_str(&yaml_content).context("解析 souls.yaml 失败")?;
        let data = outer.get("data").context("souls.yaml 缺少 data 字段")?;
        let config: SoulsConfig =
            serde_json::from_value(data.clone()).context("反序列化 SoulsConfig 失败")?;

        let llm_client = Arc::new(GovernanceLlmClient::load());
        let capability_manifest = CapabilityManifest::load();

        info!(
            "SoulReviewEngine 加载完成: {} souls, {} topic mappings, LLM enabled={}",
            config.souls.len(),
            config.topic_to_soul.len(),
            llm_client.is_enabled(),
        );
        Ok(Self {
            config,
            sources: HashMap::new(),
            llm_client,
            capability_manifest,
        })
    }

    pub fn register_source(&mut self, soul_id: &str, provider: Box<dyn SourceProvider>) {
        self.sources.insert(soul_id.to_string(), provider);
    }

    pub fn config(&self) -> &SoulsConfig {
        &self.config
    }

    // ----- routing -----

    /// 根据 governance topic 路由到对应的 primary soul
    /// 读取 config 中的 topic_to_soul 映射，不硬编码任何 soul 名称
    pub fn route_primary_soul(&self, topic: &GovernanceTopic) -> Option<String> {
        let topic_key = serde_json::to_value(topic)
            .ok()
            .and_then(|v| v.as_str().map(String::from));
        topic_key.and_then(|key| self.config.topic_to_soul.get(&key).cloned())
    }

    /// 根据 topic 列表路由，返回优先级最高的 soul
    pub fn route_for_topics(&self, topics: &[GovernanceTopic]) -> Option<String> {
        let mut best: Option<(u8, String)> = None;
        for topic in topics {
            if let Some(soul) = self.route_primary_soul(topic) {
                let priority = self
                    .config
                    .topic_priority
                    .get(
                        &serde_json::to_value(topic)
                            .ok()
                            .and_then(|v| v.as_str().map(String::from))
                            .unwrap_or_default(),
                    )
                    .copied()
                    .unwrap_or(u8::MAX);
                if best.as_ref().is_none_or(|(p, _)| priority < *p) {
                    best = Some((priority, soul));
                }
            }
        }
        best.map(|(_, soul)| soul)
    }

    // ----- policy evaluation -----

    /// 评估单条 PolicyRule
    async fn evaluate_rule(
        &self,
        rule: &PolicyRule,
        soul_id: &str,
        evidence: &ProposalEvidence,
    ) -> bool {
        match rule {
            PolicyRule::Metric {
                metric,
                operator,
                threshold,
                ..
            } => {
                let provider = self.sources.get(soul_id);
                let value = match provider {
                    Some(p) => p.get_metric(metric).await.unwrap_or(0.0),
                    None => {
                        warn!(
                            "Soul {} 没有注册 metric source，metric {} 返回默认值 0.0",
                            soul_id, metric
                        );
                        0.0
                    }
                };
                match operator.as_str() {
                    ">" => value > *threshold,
                    ">=" => value >= *threshold,
                    "<" => value < *threshold,
                    "<=" => value <= *threshold,
                    "==" | "=" => (value - *threshold).abs() < f64::EPSILON,
                    "!=" => (value - *threshold).abs() >= f64::EPSILON,
                    _ => {
                        warn!("未知 operator: {}", operator);
                        false
                    }
                }
            }
            PolicyRule::EffectRef {
                effect_ref_matches, ..
            } => {
                let provider = self.sources.get(soul_id);
                effect_ref_matches.iter().any(|pattern| match provider {
                    Some(p) => p.match_effect_ref(&evidence.ir.effect_refs, pattern),
                    None => evidence.ir.effect_refs.iter().any(|e| e == pattern),
                })
            }
            PolicyRule::EffectGroup { all_effects_in } => all_effects_in
                .iter()
                .all(|cap| evidence.ir.effect_refs.iter().any(|e| e == cap)),
        }
    }

    /// 对单个 soul 的完整 review（评估 hard_reject / hard_approve / soft_concern / LLM soft review）
    pub async fn review(
        &self,
        soul_id: &str,
        evidence: &ProposalEvidence,
        role: ReviewRole,
    ) -> ReviewVerdict {
        let soul_config = match self.config.souls.get(soul_id) {
            Some(c) => c,
            None => {
                return ReviewVerdict {
                    soul: soul_id.to_string(),
                    vote: VoteChoice::Abstain,
                    rationale: format!("Soul {} 未在配置中注册", soul_id),
                    evidence_refs: vec![],
                };
            }
        };

        // 评估 hard_reject_if
        for rule in &soul_config.review_policy.hard_reject_if {
            if self.evaluate_rule(rule, soul_id, evidence).await {
                let reason = match rule {
                    PolicyRule::Metric { reason, .. } => reason.clone(),
                    PolicyRule::EffectRef { reason, .. } => reason.clone(),
                    PolicyRule::EffectGroup { .. } => "EffectGroup 匹配".to_string(),
                };
                return ReviewVerdict {
                    soul: soul_id.to_string(),
                    vote: VoteChoice::Reject,
                    rationale: reason,
                    evidence_refs: evidence.ir.effect_refs.clone(),
                };
            }
        }

        // 评估 hard_approve_if
        for rule in &soul_config.review_policy.hard_approve_if {
            if self.evaluate_rule(rule, soul_id, evidence).await {
                let reason = match rule {
                    PolicyRule::Metric { reason, .. } => reason.clone(),
                    PolicyRule::EffectRef { reason, .. } => reason.clone(),
                    PolicyRule::EffectGroup { .. } => "EffectGroup 匹配".to_string(),
                };
                return ReviewVerdict {
                    soul: soul_id.to_string(),
                    vote: VoteChoice::Approve,
                    rationale: reason,
                    evidence_refs: evidence.ir.effect_refs.clone(),
                };
            }
        }

        // 评估 soft_concern_if（warn 级软规则，标记但不阻断）
        let mut soft_concerns: Vec<String> = Vec::new();
        for rule in &soul_config.review_policy.soft_concern_if {
            if self.evaluate_rule(rule, soul_id, evidence).await {
                let reason = match rule {
                    PolicyRule::Metric { reason, .. } => reason.clone(),
                    PolicyRule::EffectRef { reason, .. } => reason.clone(),
                    PolicyRule::EffectGroup { .. } => "EffectGroup 匹配".to_string(),
                };
                soft_concerns.push(reason);
            }
        }

        // 无硬规则命中 → LLM soft review
        let role_instruction = match role {
            ReviewRole::Primary => {
                "你是首审官，负责深度分析提案的合理性。输出详细 rationale，引用真源指标。"
            }
            ReviewRole::CoReviewer => "你是复审官，基于各自真源对齐，输出简洁 rationale。",
        };
        let system_prompt = match build_soul_prompt(
            soul_id,
            &soul_config.system_prompt_template,
            &self.capability_manifest,
            evidence,
        ) {
            Ok(p) => p,
            Err(e) => {
                warn!("构建 Soul {} prompt 失败: {}, 回退 Abstain", soul_id, e);
                return ReviewVerdict {
                    soul: soul_id.to_string(),
                    vote: VoteChoice::Abstain,
                    rationale: format!("prompt 构建失败: {}", e),
                    evidence_refs: vec![],
                };
            }
        };
        let user_message = build_review_message(evidence);
        let user_message = format!("{}\n\n{}", role_instruction, user_message);

        let mut verdict = self
            .llm_client
            .review_with_llm(&system_prompt, &user_message)
            .await;
        verdict.soul = soul_id.to_string();

        // 注入 soft_concern 上下文到 rationale
        if !soft_concerns.is_empty() {
            let concern_note = format!("\n[soft_concerns: {}]", soft_concerns.join("; "));
            verdict.rationale.push_str(&concern_note);
        }

        verdict
    }

    // ----- vote resolution -----

    /// 根据配置阈值解析投票结果
    pub fn resolve_votes(&self, votes: &[ReviewVerdict]) -> ProposalStatus {
        let approve_count = votes
            .iter()
            .filter(|v| v.vote == VoteChoice::Approve)
            .count() as u8;
        let reject_count = votes
            .iter()
            .filter(|v| v.vote == VoteChoice::Reject)
            .count() as u8;

        if approve_count >= self.config.review.approve_threshold {
            ProposalStatus::Approved
        } else if reject_count >= self.config.review.reject_threshold {
            ProposalStatus::Rejected
        } else {
            ProposalStatus::UnderReview
        }
    }

    // ----- batch review -----

    /// 批量审核待处理的 proposal groups
    /// 单个 group 失败不中断整个批次
    pub async fn review_pending(
        &self,
        store: &ProposalStore,
        groups: &[PendingGroup],
    ) -> Vec<(uuid::Uuid, ProposalStatus)> {
        let mut results = Vec::new();

        for group in groups {
            match self.review_group(store, group).await {
                Ok(status) => {
                    results.push((group.id, status));
                }
                Err(e) => {
                    error!("Group {} 审核失败: {:?}，跳过继续处理", group.id, e);
                    results.push((group.id, ProposalStatus::Error));
                }
            }
        }

        results
    }

    /// 审核单个 proposal group
    async fn review_group(
        &self,
        store: &ProposalStore,
        group: &PendingGroup,
    ) -> Result<ProposalStatus> {
        let group_full = store
            .get_group(group.id)
            .await
            .context("获取 group 详情失败")?
            .context("Group 不存在")?;

        // 确定 primary soul
        let primary_soul = match &group_full.primary_soul {
            Some(s) => s.clone(),
            None => match self.route_for_topics(&group_full.governance_topics) {
                Some(soul) => soul,
                None => {
                    warn!("Group {} 无法路由到 primary soul，使用 fallback", group.id);
                    self.config.classifier.default_fallback_topic.clone()
                }
            },
        };

        // 收集投票
        let mut votes: Vec<ReviewVerdict> = Vec::new();

        // Primary soul 评估
        for proposal_id in &group_full.proposal_ids {
            let evidence = self.build_evidence_from_group(store, *proposal_id).await?;
            let verdict = self
                .review(&primary_soul, &evidence, ReviewRole::Primary)
                .await;
            if !matches!(verdict.vote, VoteChoice::Abstain) {
                votes.push(verdict);
            }
        }

        // Co-reviewers 评估
        for co_soul in &group_full.co_reviewers {
            for proposal_id in &group_full.proposal_ids {
                let evidence = self.build_evidence_from_group(store, *proposal_id).await?;
                let verdict = self
                    .review(co_soul, &evidence, ReviewRole::CoReviewer)
                    .await;
                if !matches!(verdict.vote, VoteChoice::Abstain) {
                    votes.push(verdict);
                }
            }
        }

        let status = self.resolve_votes(&votes);

        // 检查 dissent log 阈值
        let dissent_count = group_full.dissent_log.len() as u32;
        if dissent_count >= self.config.review.dissent_log_threshold {
            return Ok(ProposalStatus::EscalatedAdmin);
        }

        // 更新 group 状态
        store
            .update_group_status(group.id, status)
            .await
            .context("更新 group 状态失败")?;

        if status == ProposalStatus::Approved {
            let approve_count = votes
                .iter()
                .filter(|v| v.vote == VoteChoice::Approve)
                .count();
            let total_votes = votes.len();
            info!(
                group_id = %group.id,
                primary_soul = %primary_soul,
                approve_count = approve_count,
                total_votes = total_votes,
                proposal_count = group_full.proposal_ids.len(),
                "Proposal group approved — auto-evolving action configs"
            );

            // Auto-evolve: 从每个 proposal 的 IR 生成 action config 并写入 actions.yaml
            let config_dir = crate::paths::get_config_dir();
            for proposal_id in &group_full.proposal_ids {
                match self.build_evidence_from_group(store, *proposal_id).await {
                    Ok(evidence) => match super::auto_evolve::generate_action_config(&evidence) {
                        Ok((action_name, entry)) => {
                            if let Err(e) = super::action_writer::append_action_to_yaml(
                                &config_dir,
                                &action_name,
                                &entry,
                            ) {
                                error!(
                                    action_name = %action_name,
                                    error = %e,
                                    "auto-evolve: 写入 actions.yaml 失败"
                                );
                            }
                        }
                        Err(e) => {
                            warn!(
                                proposal_id = %proposal_id,
                                error = %e,
                                "auto-evolve: 生成 action config 失败"
                            );
                        }
                    },
                    Err(e) => {
                        warn!(
                            proposal_id = %proposal_id,
                            error = %e,
                            "auto-evolve: 获取 proposal evidence 失败"
                        );
                    }
                }
            }
        }

        Ok(status)
    }

    async fn build_evidence_from_group(
        &self,
        store: &ProposalStore,
        proposal_id: uuid::Uuid,
    ) -> Result<ProposalEvidence> {
        store
            .get_proposal(proposal_id)
            .await
            .context("获取 proposal evidence 失败")?
            .ok_or_else(|| anyhow::anyhow!("Proposal {} not found", proposal_id))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governance::types::{
        ReviewPolicy, SoulConfig, SoulsClassifierConfig, SoulsReviewConfig,
    };

    /// Phase 0 测试配置：仅伏羲
    fn test_souls_config() -> SoulsConfig {
        let mut souls = HashMap::new();
        souls.insert(
            "fuxi".to_string(),
            SoulConfig {
                display_name: "伏羲".to_string(),
                governance_role: "evolution".to_string(),
                review_policy: ReviewPolicy::default(),
                source_bindings: HashMap::new(),
                system_prompt_template: "fuxi_review_prompt".to_string(),
            },
        );

        SoulsConfig {
            souls,
            topic_to_soul: [("evolution".to_string(), "fuxi".to_string())]
                .into_iter()
                .collect(),
            topic_priority: [("evolution".to_string(), 0)].into_iter().collect(),
            classifier: SoulsClassifierConfig {
                confidence_threshold: 0.6,
                default_fallback_topic: "evolution".to_string(),
            },
            review: SoulsReviewConfig {
                timeout_secs: 1800,
                dissent_log_threshold: 3,
                approve_threshold: 2,
                reject_threshold: 2,
                poll_interval_secs: 60,
            },
        }
    }

    fn test_engine() -> SoulReviewEngine {
        SoulReviewEngine {
            config: test_souls_config(),
            sources: HashMap::new(),
            llm_client: Arc::new(GovernanceLlmClient {
                enabled: false,
                config: None,
            }),
            capability_manifest: CapabilityManifest::default(),
        }
    }

    #[test]
    fn test_route_primary_soul() {
        let engine = test_engine();
        assert_eq!(
            engine.route_primary_soul(&GovernanceTopic::Evolution),
            Some("fuxi".to_string())
        );
        // Phase 0: 仅伏羲注册，其他 topic 返回 None
        assert_eq!(engine.route_primary_soul(&GovernanceTopic::Resource), None);
    }

    #[test]
    fn test_route_for_topics() {
        let engine = test_engine();
        // Phase 0: 仅 evolution → fuxi
        let result = engine.route_for_topics(&[GovernanceTopic::Evolution]);
        assert_eq!(result, Some("fuxi".to_string()));
    }

    #[test]
    fn test_resolve_votes_approved() {
        let engine = test_engine();
        let votes = vec![
            ReviewVerdict {
                soul: "fuxi".to_string(),
                vote: VoteChoice::Approve,
                rationale: String::new(),
                evidence_refs: vec![],
            },
            ReviewVerdict {
                soul: "fuxi".to_string(),
                vote: VoteChoice::Approve,
                rationale: String::new(),
                evidence_refs: vec![],
            },
        ];
        assert_eq!(engine.resolve_votes(&votes), ProposalStatus::Approved);
    }

    #[test]
    fn test_resolve_votes_rejected() {
        let engine = test_engine();
        let votes = vec![
            ReviewVerdict {
                soul: "fuxi".to_string(),
                vote: VoteChoice::Reject,
                rationale: String::new(),
                evidence_refs: vec![],
            },
            ReviewVerdict {
                soul: "fuxi".to_string(),
                vote: VoteChoice::Reject,
                rationale: String::new(),
                evidence_refs: vec![],
            },
        ];
        assert_eq!(engine.resolve_votes(&votes), ProposalStatus::Rejected);
    }

    #[test]
    fn test_resolve_votes_under_review() {
        let engine = test_engine();
        let votes = vec![ReviewVerdict {
            soul: "fuxi".to_string(),
            vote: VoteChoice::Approve,
            rationale: String::new(),
            evidence_refs: vec![],
        }];
        assert_eq!(engine.resolve_votes(&votes), ProposalStatus::UnderReview);
    }

    #[test]
    fn test_resolve_votes_empty() {
        let engine = test_engine();
        assert_eq!(engine.resolve_votes(&[]), ProposalStatus::UnderReview);
    }
}
