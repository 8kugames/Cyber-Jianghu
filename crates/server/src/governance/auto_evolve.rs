use anyhow::Result;
use tracing::info;

use super::types::{InferredActionConfig, ProposalEvidence, ReviewVerdict};

/// 从伏羲 LLM 审议结果 + 提案证据生成 actions.yaml 条目
///
/// 接受 ReviewVerdict（含 LLM 推断的 action_config）而非裸 evidence，
/// 确保写入 actions.yaml 的字段值来自伏羲 LLM 推断而非 agent 自报。
///
/// 返回 `(action_name, entry_value)`，caller 负责写入文件。
pub fn generate_action_config(
    evidence: &ProposalEvidence,
    verdict: &ReviewVerdict,
) -> Result<(String, serde_yaml::Value)> {
    let action_name = evidence.proposed_action_type.clone();
    let inferred: &InferredActionConfig =
        verdict.inferred_action_config.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "approve 的 verdict 缺少 inferred_action_config，无法生成 actions.yaml 条目"
            )
        })?;

    let category = infer_category(&inferred.effect_refs);
    let description = if evidence.rationale.is_empty() {
        format!("auto-evolved from proposal by agent {}", evidence.agent_id)
    } else {
        evidence.rationale.clone()
    };
    let requirements = build_requirements(&inferred.requirement_refs);

    let entry_json = serde_json::json!({
        "name": action_name,
        "description": description,
        "category": category,
        "ooc_risk": "medium",
        "transmission": "silent",
        "atomic_kind": inferred.atomic_kind,
        "actor_arity": inferred.actor_arity,
        "target_arity": inferred.target_arity,
        "tick_span": inferred.tick_span,
        "phase_count": inferred.phase_count,
        "protocol_kind": inferred.protocol_kind,
        "validation": {
            "required_fields": [],
        },
        "requirements": requirements,
    });
    let entry = serde_yaml::to_value(&entry_json)
        .map_err(|e| anyhow::anyhow!("serde_yaml::to_value 失败: {}", e))?;

    info!(
        action_name = %action_name,
        category = %category,
        atomic_kind = ?inferred.atomic_kind,
        effect_refs = ?inferred.effect_refs,
        requirement_refs = ?inferred.requirement_refs,
        "auto_evolve: 生成 action config"
    );

    Ok((action_name, entry))
}

fn infer_category(effect_refs: &[String]) -> String {
    for ref_ in effect_refs {
        if ref_.starts_with("combat") || ref_.starts_with("martial") {
            return "combat".to_string();
        }
        if ref_.starts_with("social") || ref_.starts_with("dialogue") {
            return "social".to_string();
        }
        if ref_.starts_with("craft") || ref_.starts_with("economic") {
            return "economic".to_string();
        }
    }
    "survival".to_string()
}

fn build_requirements(requirement_refs: &[String]) -> Vec<serde_yaml::Value> {
    let mut reqs: Vec<serde_yaml::Value> = Vec::new();

    let default_req = serde_json::json!({
        "requirement_type": "attribute",
        "attribute": "stamina",
        "min": 1,
        "cost": 1,
    });
    reqs.push(serde_yaml::to_value(&default_req).unwrap_or(serde_yaml::Value::Null));

    for ref_ in requirement_refs {
        let (req_type, target) = if ref_.starts_with("tool.") || ref_.starts_with("skill.") {
            ("item", ref_.as_str())
        } else {
            ("attribute", ref_.as_str())
        };

        let req = serde_json::json!({
            "requirement_type": req_type,
            "target": target,
        });
        if let Ok(v) = serde_yaml::to_value(&req) {
            reqs.push(v);
        }
    }

    reqs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governance::types::{
        InferredActionConfig, ProposalEvidence, ReviewVerdict, VoteChoice,
    };
    use cyber_jianghu_protocol::types::governance::{AtomicKind, ProtocolKind, TargetArity};
    use std::collections::HashMap;

    fn make_evidence() -> ProposalEvidence {
        ProposalEvidence {
            agent_id: uuid::Uuid::new_v4(),
            tick_id: 1,
            proposed_action_type: "test_action".to_string(),
            action_data: serde_json::json!({}),
            governance_topics: vec![],
            topic_confidence: HashMap::new(),
            rationale: "test rationale".to_string(),
        }
    }

    fn make_verdict(effect_refs: Vec<String>, requirement_refs: Vec<String>) -> ReviewVerdict {
        ReviewVerdict {
            soul: "fuxi".to_string(),
            vote: VoteChoice::Approve,
            rationale: "approved".to_string(),
            evidence_refs: vec![],
            reject_reason: None,
            inferred_action_config: Some(InferredActionConfig {
                atomic_kind: AtomicKind::Atomic,
                actor_arity: 1,
                target_arity: TargetArity::Zero,
                tick_span: 0,
                phase_count: 1,
                protocol_kind: ProtocolKind::None,
                effect_refs,
                requirement_refs,
            }),
        }
    }

    #[test]
    fn test_infer_category_combat() {
        let evidence = make_evidence();
        let verdict = make_verdict(vec!["combat.slash".into()], vec![]);
        let (name, entry) = generate_action_config(&evidence, &verdict).unwrap();
        assert_eq!(name, "test_action");
        assert_eq!(entry["category"], "combat");
    }

    #[test]
    fn test_infer_category_social() {
        let evidence = make_evidence();
        let verdict = make_verdict(vec!["dialogue.negotiate".into()], vec![]);
        let (_, entry) = generate_action_config(&evidence, &verdict).unwrap();
        assert_eq!(entry["category"], "social");
    }

    #[test]
    fn test_infer_category_economic() {
        let evidence = make_evidence();
        let verdict = make_verdict(vec!["craft.forge".into()], vec![]);
        let (_, entry) = generate_action_config(&evidence, &verdict).unwrap();
        assert_eq!(entry["category"], "economic");
    }

    #[test]
    fn test_infer_category_default() {
        let evidence = make_evidence();
        let verdict = make_verdict(vec!["misc.something".into()], vec![]);
        let (_, entry) = generate_action_config(&evidence, &verdict).unwrap();
        assert_eq!(entry["category"], "survival");
    }

    #[test]
    fn test_build_requirements_with_tool_ref() {
        let evidence = make_evidence();
        let verdict = make_verdict(vec![], vec!["tool.sword".into()]);
        let (_, entry) = generate_action_config(&evidence, &verdict).unwrap();
        let reqs = entry["requirements"].as_sequence().unwrap();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[1]["requirement_type"], "item");
        assert_eq!(reqs[1]["target"], "tool.sword");
    }

    #[test]
    fn test_empty_rationale_uses_fallback() {
        let mut evidence = make_evidence();
        evidence.rationale = String::new();
        let verdict = make_verdict(vec![], vec![]);
        let (_, entry) = generate_action_config(&evidence, &verdict).unwrap();
        let desc = entry["description"].as_str().unwrap();
        assert!(desc.contains("auto-evolved"));
    }

    #[test]
    fn test_missing_inferred_action_config_errors() {
        let evidence = make_evidence();
        let verdict = ReviewVerdict {
            soul: "fuxi".to_string(),
            vote: VoteChoice::Approve,
            rationale: "approved".to_string(),
            evidence_refs: vec![],
            reject_reason: None,
            inferred_action_config: None,
        };
        let result = generate_action_config(&evidence, &verdict);
        assert!(result.is_err());
    }

    #[test]
    fn test_atomic_kind_written_to_yaml() {
        let evidence = make_evidence();
        let verdict = make_verdict(vec![], vec![]);
        let (_, entry) = generate_action_config(&evidence, &verdict).unwrap();
        assert_eq!(entry["atomic_kind"], "atomic");
        assert_eq!(entry["actor_arity"], 1);
        assert_eq!(entry["tick_span"], 0);
    }
}
