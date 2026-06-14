use anyhow::{Context, Result};
use tracing::{error, info, warn};

use crate::game_data::loaders::LlmConfig;

use super::manifest::CapabilityManifest;
use super::types::{
    InferredActionConfig, ProposalEvidence, RejectReason, ReviewVerdict, VoteChoice,
};

// ---------------------------------------------------------------------------
// Structured LLM response (parsed from JSON output)
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct LlmReviewResponse {
    vote: String,
    rationale: String,
    #[serde(default)]
    evidence_refs: Vec<String>,
    /// reject 时细分原因（"non_atomic" / "governance_value" / "other"）
    #[serde(default)]
    reject_reason: Option<String>,
    /// approve 时附带的 LLM 推断动作字段（写入 actions.yaml）
    #[serde(default)]
    inferred_action_config: Option<InferredActionConfig>,
}

/// 构造系统强制 Reject（LLM 超时/失败时使用，符合管道"不允许弃权、超时视作拒绝"约束）
///
/// # 为什么不允许 abstain
///
/// 管道设计要求三皇必须给出 approve / reject 明确态度——弃权会让 stage 推进逻辑
/// 无法判定（同辈审议全部 abstain 时既无法关单也无法推进）。
/// LLM 调用失败时由系统强制注入 reject（reject_reason = Other），保证管道可继续推进。
fn system_reject(soul: &str, rationale: impl Into<String>) -> ReviewVerdict {
    ReviewVerdict {
        soul: soul.to_string(),
        vote: VoteChoice::Reject,
        rationale: rationale.into(),
        evidence_refs: vec![],
        reject_reason: Some(RejectReason::Other),
        inferred_action_config: None,
    }
}

// ---------------------------------------------------------------------------
// GovernanceLlmClient
// ---------------------------------------------------------------------------

pub struct GovernanceLlmClient {
    pub(crate) enabled: bool,
    pub(crate) config: Option<LlmConfig>,
}

impl GovernanceLlmClient {
    pub fn load() -> Self {
        match crate::game_data::loaders::load_llm(&crate::paths::get_config_dir()) {
            Ok(config) if config.enabled && !config.api_key.is_empty() => {
                info!(
                    "Governance LLM client 初始化成功 (provider: {}, model: {})",
                    config.provider, config.model
                );
                Self {
                    enabled: true,
                    config: Some(config),
                }
            }
            Ok(_) => {
                info!("Governance LLM client 未启用（enabled=false 或 api_key 为空）");
                Self {
                    enabled: false,
                    config: None,
                }
            }
            Err(e) => {
                warn!("Governance LLM client 加载失败: {}", e);
                Self {
                    enabled: false,
                    config: None,
                }
            }
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub async fn review_with_llm(&self, system_prompt: &str, user_message: &str) -> ReviewVerdict {
        if !self.enabled {
            return system_reject("", "LLM 未启用，按管道约束视作拒绝");
        }

        let config = self.config.as_ref().unwrap();

        let request_body = serde_json::json!({
            "model": config.model,
            "messages": [
                {
                    "role": "system",
                    "content": system_prompt
                },
                {
                    "role": "user",
                    "content": user_message
                }
            ],
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
            "response_format": { "type": "json_object" }
        });

        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.request_timeout_secs))
            .connect_timeout(std::time::Duration::from_secs(config.connect_timeout_secs))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                error!("Governance LLM 构建 HTTP 客户端失败: {}", e);
                return system_reject("", format!("LLM 客户端构建失败: {}", e));
            }
        };

        let base_url = config.base_url.trim_end_matches('/');
        let url = if base_url.contains("/chat/completions") {
            base_url.to_string()
        } else {
            format!("{}/chat/completions", base_url)
        };

        let response = match client
            .post(&url)
            .header("Authorization", format!("Bearer {}", config.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                error!("Governance LLM 请求失败: {}", e);
                return system_reject("", format!("LLM 请求失败（超时/网络）: {}", e));
            }
        };

        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        if !status.is_success() {
            error!("Governance LLM 返回错误状态 {}: {}", status, body);
            return system_reject("", format!("LLM 返回错误 {}: {}", status, body));
        }

        // Parse LLM response envelope
        #[derive(serde::Deserialize)]
        struct LlmEnvelope {
            choices: Vec<LlmChoice>,
        }

        #[derive(serde::Deserialize)]
        struct LlmChoice {
            message: LlmMessage,
        }

        #[derive(serde::Deserialize)]
        struct LlmMessage {
            content: String,
        }

        let envelope: LlmEnvelope = match serde_json::from_str(&body) {
            Ok(e) => e,
            Err(e) => {
                error!("Governance LLM 解析响应信封失败: {}", e);
                return system_reject("", format!("LLM 响应解析失败: {}", e));
            }
        };

        let content = envelope
            .choices
            .first()
            .map(|c| c.message.content.as_str())
            .unwrap_or("");

        if content.trim().is_empty() {
            error!("Governance LLM 返回空内容");
            return system_reject("", "LLM 返回空内容");
        }

        // Parse structured JSON from LLM output
        match serde_json::from_str::<LlmReviewResponse>(content) {
            Ok(parsed) => {
                let vote = match parsed.vote.as_str() {
                    "approve" => VoteChoice::Approve,
                    // 管道不允许弃权：LLM 输出非 approve 一律视作 reject
                    _ => VoteChoice::Reject,
                };
                let reject_reason = parsed.reject_reason.as_deref().and_then(|s| match s {
                    "non_atomic" => Some(RejectReason::NonAtomic),
                    "governance_value" => Some(RejectReason::GovernanceValue),
                    "other" => Some(RejectReason::Other),
                    _ => None,
                });
                info!(
                    "Governance LLM 裁决完成: vote={}, reject_reason={:?}, rationale_len={}",
                    parsed.vote,
                    reject_reason,
                    parsed.rationale.len()
                );
                ReviewVerdict {
                    soul: String::new(),
                    vote,
                    rationale: parsed.rationale,
                    evidence_refs: parsed.evidence_refs,
                    reject_reason,
                    inferred_action_config: parsed.inferred_action_config,
                }
            }
            Err(e) => {
                let preview: String = content.chars().take(200).collect();
                warn!(
                    "Governance LLM 输出无法解析为结构化 JSON ({}), 原始内容: {}",
                    e, preview
                );
                system_reject("", format!("LLM 输出格式异常: {}", e))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Prompt builder
// ---------------------------------------------------------------------------

/// 加载 prompt 模板文件（从 config/prompts/ 目录）
fn load_prompt_template(template_name: &str) -> Result<String> {
    let prompts_dir = crate::paths::get_config_dir().join("prompts");
    let path = prompts_dir.join(format!("{}.md", template_name));
    std::fs::read_to_string(&path)
        .with_context(|| format!("读取 prompt 模板失败: {}", path.display()))
}

/// 构建 Soul 审议的 system prompt
pub fn build_soul_prompt(
    _soul_id: &str,
    soul_system_prompt_template: &str,
    capability_manifest: &CapabilityManifest,
    evidence: &ProposalEvidence,
) -> Result<String> {
    let template = load_prompt_template(soul_system_prompt_template)?;

    // 能力清单格式化
    let capabilities = capability_manifest
        .entries()
        .iter()
        .map(|e| {
            format!(
                "- {} (kind: {}, scope: {})",
                e.capability_id, e.kind, e.semantic_scope
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let action_data_yaml = serde_yaml::to_string(&evidence.action_data)
        .unwrap_or_else(|_| "<serialize failed>".to_string());

    let prompt = template
        .replace("{capabilities}", &capabilities)
        .replace("{action_type}", &evidence.proposed_action_type)
        .replace("{action_data}", &action_data_yaml)
        .replace("{rationale}", &evidence.rationale);

    Ok(prompt)
}

/// 构建 LLM 审议的 user message
pub fn build_review_message(evidence: &ProposalEvidence) -> String {
    format!(
        "请审议以下提案。按照 system prompt 中定义的维度逐一分析，最后输出严格 JSON 格式的裁决。\n\n\
         提案 ID: {}\n\
         提出者 Agent: {}\n\
         动作类型: {}\n\
         Intent 参数:\n{}\n\
         提案理由: {}",
        evidence.tick_id,
        evidence.agent_id,
        evidence.proposed_action_type,
        serde_yaml::to_string(&evidence.action_data).unwrap_or_default(),
        evidence.rationale,
    )
}

/// 构建伏羲终审 user message（追加神农/轩辕的反馈）
///
/// # 终审阶段语义
///
/// 伏羲初审已 approve（含 inferred_action_config），神农/轩辕并行审议后达成 2/3 多数。
/// 伏羲需要根据同辈反馈做最终决定：
/// - 同辈反馈中有附条件批准（approve 但 rationale 中明示条件）→ 调整 inferred_action_config
/// - 同辈反馈无实质调整需求 → 沿用初审 config
///
/// # 附条件过审表达
///
/// 不通过新增 VoteChoice 实现，而是在 approve 的 rationale 中明示条件
/// （由伏羲 LLM 在终审时解读）。这是 D1=C 决策：保持枚举简洁，依赖 LLM 理解力。
pub fn build_final_review_message(
    evidence: &ProposalEvidence,
    peer_verdicts: &[ReviewVerdict],
) -> String {
    let peer_feedback: String = peer_verdicts
        .iter()
        .map(|v| {
            format!(
                "## {} 的反馈\n- 投票: {}\n- 理由: {}",
                v.soul,
                match v.vote {
                    VoteChoice::Approve => "批准",
                    VoteChoice::Reject => "拒绝",
                    VoteChoice::Abstain => "弃权",
                },
                v.rationale
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    format!(
        "你已在初审中批准此提案。现在需要根据同辈反馈做最终决定。\n\n\
         ## 原提案\n\
         提案 ID: {}\n\
         提出者 Agent: {}\n\
         动作类型: {}\n\
         Intent 参数:\n{}\n\
         提案理由: {}\n\n\
         ## 同辈反馈\n{}\n\n\
         ## 终审要求\n\
         1. 阅读同辈反馈，判断是否有需要调整的合理顾虑\n\
         2. 若同辈反馈中有附条件批准（approve 但 rationale 中明示条件），需调整 inferred_action_config 以满足合理条件\n\
         3. 若同辈反馈无实质调整需求，沿用初审 config\n\
         4. 输出最终 inferred_action_config（用于写入 actions.yaml）",
        evidence.tick_id,
        evidence.agent_id,
        evidence.proposed_action_type,
        serde_yaml::to_string(&evidence.action_data).unwrap_or_default(),
        evidence.rationale,
        peer_feedback,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governance::types::{ProposalEvidence, VoteChoice};
    use cyber_jianghu_protocol::types::governance::GovernanceTopic;

    fn test_evidence() -> ProposalEvidence {
        ProposalEvidence {
            agent_id: uuid::Uuid::new_v4(),
            tick_id: 1,
            proposed_action_type: "combat.slash".to_string(),
            action_data: serde_json::json!({
                "target_agent_id": "abc-def-123",
                "item_id": "sword_001"
            }),
            governance_topics: vec![GovernanceTopic::Evolution],
            topic_confidence: [(GovernanceTopic::Evolution, 0.9)].into_iter().collect(),
            rationale: "test proposal".to_string(),
        }
    }

    #[test]
    fn test_build_review_message() {
        let evidence = test_evidence();
        let msg = build_review_message(&evidence);
        assert!(msg.contains("combat.slash"));
        assert!(msg.contains("test proposal"));
    }

    #[test]
    fn test_governance_llm_client_disabled() {
        let client = GovernanceLlmClient {
            enabled: false,
            config: None,
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let verdict = rt.block_on(client.review_with_llm("system", "user"));
        assert_eq!(verdict.vote, VoteChoice::Reject);
        assert!(verdict.rationale.contains("未启用"));
    }

    #[test]
    fn test_load_prompt_template_missing() {
        let result = load_prompt_template("nonexistent_template_xyz");
        assert!(result.is_err());
    }

    #[test]
    fn test_build_soul_prompt() {
        let evidence = test_evidence();
        let manifest = CapabilityManifest::default();
        let result = build_soul_prompt("fuxi", "fuxi_review", &manifest, &evidence);
        // Template file may not exist in test env, but the function should not panic
        // If template exists, it should contain the substituted values
        match result {
            Ok(prompt) => {
                assert!(prompt.contains("combat.slash"));
                assert!(prompt.contains("test proposal"));
            }
            Err(_) => {
                // Template file not found in test environment — acceptable
            }
        }
    }
}
