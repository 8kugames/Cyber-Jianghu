use anyhow::{Context, Result};
use tracing::{error, info, warn};

use crate::game_data::loaders::LlmConfig;

use super::manifest::CapabilityManifest;
use super::types::{ProposalEvidence, ReviewVerdict, VoteChoice};

// ---------------------------------------------------------------------------
// Structured LLM response (parsed from JSON output)
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct LlmReviewResponse {
    vote: String,
    rationale: String,
    #[serde(default)]
    evidence_refs: Vec<String>,
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
            return ReviewVerdict {
                soul: String::new(),
                vote: VoteChoice::Abstain,
                rationale: "LLM 未启用，无法执行软裁决".to_string(),
                evidence_refs: vec![],
            };
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
                return ReviewVerdict {
                    soul: String::new(),
                    vote: VoteChoice::Abstain,
                    rationale: format!("LLM 客户端构建失败: {}", e),
                    evidence_refs: vec![],
                };
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
                return ReviewVerdict {
                    soul: String::new(),
                    vote: VoteChoice::Abstain,
                    rationale: format!("LLM 请求失败: {}", e),
                    evidence_refs: vec![],
                };
            }
        };

        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        if !status.is_success() {
            error!("Governance LLM 返回错误状态 {}: {}", status, body);
            return ReviewVerdict {
                soul: String::new(),
                vote: VoteChoice::Abstain,
                rationale: format!("LLM 返回错误 {}: {}", status, body),
                evidence_refs: vec![],
            };
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
                return ReviewVerdict {
                    soul: String::new(),
                    vote: VoteChoice::Abstain,
                    rationale: format!("LLM 响应解析失败: {}", e),
                    evidence_refs: vec![],
                };
            }
        };

        let content = envelope
            .choices
            .first()
            .map(|c| c.message.content.as_str())
            .unwrap_or("");

        if content.trim().is_empty() {
            error!("Governance LLM 返回空内容");
            return ReviewVerdict {
                soul: String::new(),
                vote: VoteChoice::Abstain,
                rationale: "LLM 返回空内容".to_string(),
                evidence_refs: vec![],
            };
        }

        // Parse structured JSON from LLM output
        match serde_json::from_str::<LlmReviewResponse>(content) {
            Ok(parsed) => {
                let vote = match parsed.vote.as_str() {
                    "approve" => VoteChoice::Approve,
                    "reject" => VoteChoice::Reject,
                    _ => VoteChoice::Abstain,
                };
                info!(
                    "Governance LLM 裁决完成: vote={}, rationale_len={}",
                    parsed.vote,
                    parsed.rationale.len()
                );
                ReviewVerdict {
                    soul: String::new(),
                    vote,
                    rationale: parsed.rationale,
                    evidence_refs: parsed.evidence_refs,
                }
            }
            Err(e) => {
                let preview: String = content.chars().take(200).collect();
                warn!(
                    "Governance LLM 输出无法解析为结构化 JSON ({}), 原始内容: {}",
                    e,
                    preview
                );
                ReviewVerdict {
                    soul: String::new(),
                    vote: VoteChoice::Abstain,
                    rationale: format!("LLM 输出格式异常: {}", e),
                    evidence_refs: vec![],
                }
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

    let effect_refs = evidence
        .ir
        .effect_refs
        .iter()
        .map(|r| format!("- {}", r))
        .collect::<Vec<_>>()
        .join("\n");

    let requirement_refs = evidence
        .ir
        .requirement_refs
        .iter()
        .map(|r| format!("- {}", r))
        .collect::<Vec<_>>()
        .join("\n");

    let prompt = template
        .replace("{capabilities}", &capabilities)
        .replace("{action_type}", &evidence.proposed_action_type)
        .replace("{effect_refs}", &effect_refs)
        .replace("{requirement_refs}", &requirement_refs)
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
         效果引用: {:?}\n\
         前置条件: {:?}\n\
         提案理由: {}",
        evidence.tick_id,
        evidence.agent_id,
        evidence.proposed_action_type,
        evidence.ir.effect_refs,
        evidence.ir.requirement_refs,
        evidence.rationale,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governance::types::{ProposalEvidence, VoteChoice};
    use cyber_jianghu_protocol::types::governance::{GovernanceTopic, ProposedActionIR};

    fn test_evidence() -> ProposalEvidence {
        ProposalEvidence {
            agent_id: uuid::Uuid::new_v4(),
            tick_id: 1,
            proposed_action_type: "combat.slash".to_string(),
            ir: ProposedActionIR {
                source: cyber_jianghu_protocol::types::governance::IRSource::FromAgentIntent,
                atomic_kind: cyber_jianghu_protocol::types::governance::AtomicKind::Unknown,
                actor_arity: 1,
                target_arity: cyber_jianghu_protocol::types::governance::TargetArity::One,
                tick_span: 0,
                phase_count: 1,
                protocol_kind: cyber_jianghu_protocol::types::governance::ProtocolKind::None,
                effect_refs: vec!["combat.slash".into()],
                requirement_refs: vec!["tool.sword".into()],
            },
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
        assert_eq!(verdict.vote, VoteChoice::Abstain);
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
