//! 三皇共审引擎 —— 火云洞天宏观治理智能的动作演化治理管道
//!
//! # 管道流程
//!
//! ```text
//! proposal_group 创建（stage = awaiting_fuxi_initial）
//!         ↓
//! 阶段 1：伏羲初审（stage_fuxi_initial）
//!   ├─ 拒绝 → update(AwaitingFuxiInitial→Done, Rejected) → 整组关单
//!   └─ 批准（含 inferred_action_config）→ stage = awaiting_peer
//!         ↓
//! 阶段 2：神农 ‖ 轩辕并行（stage_peer_review，tokio::join!）
//!   ├─ 全部拒绝 → update(Done, Rejected) → 整组关单
//!   └─ ≥1 票批准（total ≥ approve_threshold）→ stage = awaiting_fuxi_final
//!         ↓
//! 阶段 3：伏羲终审（stage_fuxi_final，注入同辈反馈）
//!   ├─ dissent_log 阈值检查 → 升级 EscalatedAdmin
//!   ├─ 写入 actions.yaml 失败 → return Err，stage 保持 awaiting_fuxi_final 等下轮重试
//!   └─ 写入成功 → update(Done, Approved)
//! ```
//!
//! # 设计约束
//!
//! - **不允许弃权**：LLM 超时/失败由系统强制注入 Reject（`llm_review::system_reject`）
//! - **同 similarity_key 多 proposal 共享 fate**：每 group 取首个 proposal 作为审议样本，
//!   通过则整组批准，actions.yaml 按 action_type 去重写入
//! - **stage 持久化**：管道阶段进度存 DB，重启后从断点继续
//! - **close_stale_groups 阶段感知**：仅关闭 `awaiting_fuxi_initial` 超时 group，
//!   已进入审议管道的 group 由轮询任务自动重试
//! - **写入失败保护**：阶段 3 写入 actions.yaml 失败时不标记 Approved，
//!   避免状态分裂（group 标 Approved 但 actions.yaml 实际未写入）
//!
//! # Error 状态恢复路径
//!
//! 阶段 3 写入 actions.yaml 失败时，`review_pending` catch Err 后标记 `ProposalStatus::Error`，
//! 但 **stage 保持 `awaiting_fuxi_final`**（未调用 update_group_status_and_stage）。
//!
//! 注意：`get_pending_groups` SQL 仅查 `status IN ('pending_review', 'under_review')`，
//! Error 状态的 group **不会**被下一轮轮询拉取。这是设计意图——避免持续失败的写入
//! 每轮都重试占用 LLM 配额。
//!
//! 恢复方式（任一即可）：
//! 1. **同名 action 重新提议**：agent 再次提交同名 action 时，`upsert_proposal_group`
//!    ON CONFLICT 检测到 `stage='done'` 会重置管道。但 Error 状态的 group stage 仍是
//!    `awaiting_fuxi_final` 而非 `done`——此路径不生效。需 DBA 介入。
//! 2. **DBA 介入**：直接 UPDATE proposal_groups SET status='under_review' WHERE id=...
//!    让轮询任务重新拉取并重试阶段 3。
//! 3. **后续优化**（未实装）：增加 `recover_error_groups()` 接口，按配置的重试策略
//!    重新拉取 Error 状态的 group。
//!
//! 当前采用方案 2（DBA 介入）——Error 状态是相对罕见的边界场景，过度自动化可能掩盖
//! 持续性故障（如 LLM 服务异常、磁盘满）。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::{error, info, warn};

use cyber_jianghu_protocol::GovernanceTopic;

use super::llm_review::{
    GovernanceLlmClient, build_final_review_message, build_review_message, build_soul_prompt,
};
use super::manifest::CapabilityManifest;
use super::proposal_store::{PendingGroup, ProposalStore};
use super::types::{
    PolicyRule, ProposalEvidence, ProposalStage, ProposalStatus, ReviewRole, ReviewVerdict,
    SoulsConfig, VoteChoice,
};

// ---------------------------------------------------------------------------
// SourceProvider trait
// ---------------------------------------------------------------------------

/// 三皇 soul_id 常量（与 souls.yaml 中的 key 一致）
///
/// 管道硬编码这三个 ID 而非从 souls.yaml 动态读取：
/// - 管道语义绑定三皇角色（伏羲初审/终审 + 神农轩辕同辈），动态读取会破坏语义
/// - souls.yaml 启用/禁用三皇是配置层职责，管道层假设三皇完整
const FUXI_SOUL_ID: &str = "fuxi";
const SHENNONG_SOUL_ID: &str = "shennong";
const XUANYUAN_SOUL_ID: &str = "xuanyuan";

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
    capability_manifest: Arc<tokio::sync::RwLock<CapabilityManifest>>,
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
        let capability_manifest = Arc::new(tokio::sync::RwLock::new(CapabilityManifest::load()));

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

    /// 重新加载 CapabilityManifest（auto-evolve 写入新 action 后调用）
    pub async fn reload_manifest(&self) {
        let new_manifest = CapabilityManifest::load();
        let count = new_manifest.entries().len();
        *self.capability_manifest.write().await = new_manifest;
        info!("SoulReviewEngine CapabilityManifest 已刷新: {} 条目", count);
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
        _evidence: &ProposalEvidence,
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
                // Phase 0：伏羲单 soul + agent 提议时无 effect_refs（LLM 审议后才有），
                // EffectRef 规则永远不命中。Phase 2 多 soul 上线时由 LLM 推断后回填。
                let _ = (effect_ref_matches, soul_id);
                false
            }
            PolicyRule::EffectGroup { all_effects_in } => {
                let _ = all_effects_in;
                false
            }
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
                return ReviewVerdict::abstain(soul_id, format!("Soul {} 未在配置中注册", soul_id));
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
                    evidence_refs: vec![],
                    reject_reason: Some(super::types::RejectReason::Other),
                    inferred_action_config: None,
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
                    evidence_refs: vec![],
                    reject_reason: None,
                    inferred_action_config: None,
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
        let manifest_guard = self.capability_manifest.read().await;
        let system_prompt = match build_soul_prompt(
            soul_id,
            &soul_config.system_prompt_template,
            &manifest_guard,
            evidence,
        ) {
            Ok(p) => p,
            Err(e) => {
                warn!("构建 Soul {} prompt 失败: {}, 回退 Abstain", soul_id, e);
                return ReviewVerdict::abstain(soul_id, format!("prompt 构建失败: {}", e));
            }
        };
        drop(manifest_guard);
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

    /// 审核单个 proposal group（三阶段管道）
    ///
    /// 阶段 1：伏羲初审 → 拒绝直接关单
    /// 阶段 2：神农 + 轩辕并行 → 全部拒绝关单 / ≥1 票批准进阶段 3
    /// 阶段 3：伏羲终审调整 + 写入 actions.yaml
    ///
    /// 同 similarity_key 的多个 proposal 共享 fate，取首个作为审议样本。
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

        info!(
            group_id = %group.id,
            stage = ?group_full.stage,
            proposal_count = group_full.proposal_ids.len(),
            "review_group 进入阶段"
        );

        match group_full.stage {
            ProposalStage::AwaitingFuxiInitial => self.stage_fuxi_initial(store, &group_full).await,
            ProposalStage::AwaitingPeer => self.stage_peer_review(store, &group_full).await,
            ProposalStage::AwaitingFuxiFinal => self.stage_fuxi_final(store, &group_full).await,
            ProposalStage::Done => Ok(group_full.status),
        }
    }

    /// 阶段 1：伏羲初审
    ///
    /// 输入：演化池中的提案（action_type + action_data + rationale）。
    /// 输出：approve（含 inferred_action_config）→ 推进到阶段 2；
    ///       reject / 超时 → 整组关单，神农与轩辕不再介入。
    ///
    /// 同 similarity_key 多 proposal 共享 fate——取首个 proposal 作为审议样本，
    /// 伏羲拒绝则整组关单（同名 action 的其他 proposal 一并标记 Rejected）。
    async fn stage_fuxi_initial(
        &self,
        store: &ProposalStore,
        group_full: &super::proposal_store::GroupFull,
    ) -> Result<ProposalStatus> {
        let Some(sample_proposal_id) = group_full.proposal_ids.first().copied() else {
            warn!(group_id = %group_full.id, "group 无 proposal，直接关单");
            store
                .update_group_status_and_stage(
                    group_full.id,
                    ProposalStatus::Rejected,
                    ProposalStage::Done,
                )
                .await?;
            return Ok(ProposalStatus::Rejected);
        };

        let evidence = self
            .build_evidence_from_group(store, sample_proposal_id)
            .await?;
        let verdict = self
            .review(FUXI_SOUL_ID, &evidence, ReviewRole::Primary)
            .await;

        self.persist_vote(store, group_full.id, &verdict, "primary")
            .await;

        if verdict.vote != VoteChoice::Approve {
            info!(
                group_id = %group_full.id,
                vote = ?verdict.vote,
                reject_reason = ?verdict.reject_reason,
                "伏羲初审未通过，整组关单"
            );
            store
                .update_group_status_and_stage(
                    group_full.id,
                    ProposalStatus::Rejected,
                    ProposalStage::Done,
                )
                .await?;
            return Ok(ProposalStatus::Rejected);
        }

        info!(group_id = %group_full.id, "伏羲初审通过，进入同辈审议");
        store
            .update_group_stage(group_full.id, ProposalStage::AwaitingPeer)
            .await?;
        Ok(ProposalStatus::UnderReview)
    }

    /// 阶段 2：神农 + 轩辕并行审议
    ///
    /// 输入：伏羲初审已 approve 的 proposal（伏羲准备的完整 action 配置）。
    /// 并行调用神农 + 轩辕（`tokio::join!`），延迟减半。
    ///
    /// 判定逻辑：
    /// - total_approve = 伏羲初审（1 票）+ 同辈批准数
    /// - total_approve ≥ approve_threshold（默认 2）→ 进入阶段 3
    /// - 否则 → 整组关单
    ///
    /// 即"两票全 reject 才关单"等价于"伏羲初审 + 至少一票同辈批准才通过"。
    async fn stage_peer_review(
        &self,
        store: &ProposalStore,
        group_full: &super::proposal_store::GroupFull,
    ) -> Result<ProposalStatus> {
        let Some(sample_proposal_id) = group_full.proposal_ids.first().copied() else {
            warn!(group_id = %group_full.id, "group 无 proposal，直接关单");
            store
                .update_group_status_and_stage(
                    group_full.id,
                    ProposalStatus::Rejected,
                    ProposalStage::Done,
                )
                .await?;
            return Ok(ProposalStatus::Rejected);
        };

        let evidence = self
            .build_evidence_from_group(store, sample_proposal_id)
            .await?;

        // 神农 + 轩辕并行审议
        let (shennong_v, xuanyuan_v) = tokio::join!(
            self.review(SHENNONG_SOUL_ID, &evidence, ReviewRole::CoReviewer),
            self.review(XUANYUAN_SOUL_ID, &evidence, ReviewRole::CoReviewer),
        );

        self.persist_vote(store, group_full.id, &shennong_v, "co_reviewer")
            .await;
        self.persist_vote(store, group_full.id, &xuanyuan_v, "co_reviewer")
            .await;

        // 判定（伏羲初审已 approve，需要至少一票同辈 approve 才达 approve_threshold=2）
        // approve_threshold 来自 souls.yaml，默认 2（伏羲初审 + 至少一票同辈）
        let peer_approve_count = [shennong_v.vote, xuanyuan_v.vote]
            .iter()
            .filter(|v| **v == VoteChoice::Approve)
            .count();
        let total_approve = 1u8 + peer_approve_count as u8; // 伏羲初审 + 同辈批准数

        if total_approve < self.config.review.approve_threshold {
            info!(
                group_id = %group_full.id,
                shennong = ?shennong_v.vote,
                xuanyuan = ?xuanyuan_v.vote,
                total_approve,
                approve_threshold = self.config.review.approve_threshold,
                "同辈审议未达 approve_threshold，整组关单"
            );
            store
                .update_group_status_and_stage(
                    group_full.id,
                    ProposalStatus::Rejected,
                    ProposalStage::Done,
                )
                .await?;
            return Ok(ProposalStatus::Rejected);
        }

        info!(
            group_id = %group_full.id,
            total_approve,
            approve_threshold = self.config.review.approve_threshold,
            "同辈审议达 approve_threshold，进入伏羲终审"
        );
        store
            .update_group_stage(group_full.id, ProposalStage::AwaitingFuxiFinal)
            .await?;
        Ok(ProposalStatus::UnderReview)
    }

    /// 阶段 3：伏羲终审调整 + 写入 actions.yaml
    ///
    /// 输入：伏羲初审 verdict（已在 votes 表）+ 神农/轩辕 peer verdicts（从 votes 表读）。
    /// 流程：
    /// 1. dissent_log 阈值检查（分歧过多 → EscalatedAdmin）
    /// 2. 构造终审 user_message：附加 peer verdicts 摘要
    /// 3. 伏羲 LLM 终审 → final_verdict（可能反转，但少见）
    /// 4. 写入 actions.yaml：失败时 return Err，stage 保持 awaiting_fuxi_final 等下轮重试，
    ///    避免状态分裂（group 标 Approved 但 actions.yaml 未写入）
    /// 5. 成功 → update(Done, Approved)
    async fn stage_fuxi_final(
        &self,
        store: &ProposalStore,
        group_full: &super::proposal_store::GroupFull,
    ) -> Result<ProposalStatus> {
        let Some(sample_proposal_id) = group_full.proposal_ids.first().copied() else {
            warn!(group_id = %group_full.id, "group 无 proposal，直接关单");
            store
                .update_group_status_and_stage(
                    group_full.id,
                    ProposalStatus::Rejected,
                    ProposalStage::Done,
                )
                .await?;
            return Ok(ProposalStatus::Rejected);
        };

        let evidence = self
            .build_evidence_from_group(store, sample_proposal_id)
            .await?;

        // dissent_log 阈值检查：分歧过多则升级管理员人工审批
        let dissent_count = group_full.dissent_log.len() as u32;
        if dissent_count >= self.config.review.dissent_log_threshold {
            warn!(
                group_id = %group_full.id,
                dissent_count,
                threshold = self.config.review.dissent_log_threshold,
                "dissent_log 达阈值，升级管理员审批"
            );
            store
                .update_group_status_and_stage(
                    group_full.id,
                    ProposalStatus::EscalatedAdmin,
                    ProposalStage::Done,
                )
                .await?;
            return Ok(ProposalStatus::EscalatedAdmin);
        }

        // 取神农 + 轩辕的 verdict 作为终审输入
        let peer_verdicts: Vec<ReviewVerdict> = group_full
            .votes
            .iter()
            .filter(|v| v.soul == SHENNONG_SOUL_ID || v.soul == XUANYUAN_SOUL_ID)
            .map(|v| ReviewVerdict {
                soul: v.soul.clone(),
                vote: v.vote,
                rationale: v.rationale.clone(),
                evidence_refs: v.evidence_refs.clone(),
                reject_reason: None,
                inferred_action_config: None,
            })
            .collect();

        // 伏羲终审：注入 peer_verdicts 到 user_message
        let fuxi_config = self
            .config
            .souls
            .get(FUXI_SOUL_ID)
            .ok_or_else(|| anyhow::anyhow!("伏羲配置缺失，管道阶段 3 无法继续"))?;
        let manifest_guard = self.capability_manifest.read().await;
        let system_prompt = build_soul_prompt(
            FUXI_SOUL_ID,
            &fuxi_config.system_prompt_template,
            &manifest_guard,
            &evidence,
        )?;
        drop(manifest_guard);
        let user_message = build_final_review_message(&evidence, &peer_verdicts);

        let mut final_verdict = self
            .llm_client
            .review_with_llm(&system_prompt, &user_message)
            .await;
        final_verdict.soul = FUXI_SOUL_ID.to_string();

        self.persist_vote(store, group_full.id, &final_verdict, "primary_final")
            .await;

        if final_verdict.vote != VoteChoice::Approve {
            // 终审反转（罕见，伏羲基于同辈反馈改变判断）
            warn!(
                group_id = %group_full.id,
                "伏羲终审反转，整组关单"
            );
            store
                .update_group_status_and_stage(
                    group_full.id,
                    ProposalStatus::Rejected,
                    ProposalStage::Done,
                )
                .await?;
            return Ok(ProposalStatus::Rejected);
        }

        // 写入 actions.yaml（同名 action 只写一次，但所有 proposal 标记 approved）
        // 失败时返回 Err，review_pending 中 catch 后标记 Error，stage 保持 awaiting_fuxi_final
        // 等下轮重试，避免 group 标 Approved 但 actions.yaml 实际未写入的状态分裂
        let config_dir = crate::paths::get_config_dir();
        let (action_name, entry) =
            super::auto_evolve::generate_action_config(&evidence, &final_verdict).map_err(|e| {
                error!(
                    group_id = %group_full.id,
                    error = %e,
                    "auto-evolve: 生成 action config 失败，标记 Error 等待重试"
                );
                anyhow::anyhow!("auto-evolve 生成失败: {}", e)
            })?;

        super::action_writer::append_action_to_yaml(&config_dir, &action_name, &entry).map_err(
            |e| {
                error!(
                    action_name = %action_name,
                    error = %e,
                    "auto-evolve: 写入 actions.yaml 失败，标记 Error 等待重试"
                );
                anyhow::anyhow!("actions.yaml 写入失败: {}", e)
            },
        )?;

        info!(group_id = %group_full.id, "三皇共审完成，整组批准");
        store
            .update_group_status_and_stage(
                group_full.id,
                ProposalStatus::Approved,
                ProposalStage::Done,
            )
            .await?;
        Ok(ProposalStatus::Approved)
    }

    /// 持久化单条 vote 到 soul_review_votes 表
    ///
    /// 写入失败仅 warn 日志，不阻塞管道（vote 表用于审计，缺失一条不影响主流程）。
    /// role_str 取值："primary"（伏羲初审）/ "co_reviewer"（神农轩辕）/ "primary_final"（伏羲终审）。
    async fn persist_vote(
        &self,
        store: &ProposalStore,
        group_id: uuid::Uuid,
        verdict: &ReviewVerdict,
        role_str: &str,
    ) {
        let vote_str = match verdict.vote {
            VoteChoice::Approve => "approve",
            VoteChoice::Reject => "reject",
            VoteChoice::Abstain => "abstain",
        };
        if let Err(e) = store
            .write_vote(
                group_id,
                &verdict.soul,
                role_str,
                vote_str,
                &verdict.rationale,
                &verdict.evidence_refs,
            )
            .await
        {
            warn!(
                group_id = %group_id,
                soul = %verdict.soul,
                error = %e,
                "写入投票记录失败"
            );
        }
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
                poll_interval_secs: 60,
                group_stale_secs: 1800,
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
            capability_manifest: Arc::new(tokio::sync::RwLock::new(CapabilityManifest::default())),
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
}
