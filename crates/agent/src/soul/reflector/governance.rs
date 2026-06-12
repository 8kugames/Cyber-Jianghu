use cyber_jianghu_protocol::{GovernanceCode, GovernanceTopic, ProposedActionIR};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvaluatorDecision {
    Drop,
    Propose,
}

#[derive(Debug, Clone)]
pub struct SelfEvaluatorOutput {
    pub decision: EvaluatorDecision,
    pub ir: Option<ProposedActionIR>,
    pub governance_topics: Vec<GovernanceTopic>,
    pub topic_confidence: HashMap<GovernanceTopic, f64>,
    pub rationale: String,
}

pub struct SelfEvaluator;

impl SelfEvaluator {
    pub fn evaluate(action_type: &str, governance_code: GovernanceCode) -> SelfEvaluatorOutput {
        match governance_code {
            GovernanceCode::UnknownAction => {
                let ir = ProposedActionIR {
                    actor_arity: 1,
                    target_arity: "zero_to_many".into(),
                    tick_span: 0,
                    phase_count: 1,
                    protocol_kind: "none".into(),
                    state_transition_count: 1,
                    effect_refs: vec![action_type.to_string()],
                    requirement_refs: vec![],
                };
                let topics = infer_topics(action_type);
                let confidence: HashMap<GovernanceTopic, f64> =
                    topics.iter().map(|t| (*t, 0.7)).collect();
                SelfEvaluatorOutput {
                    decision: EvaluatorDecision::Propose,
                    ir: Some(ir),
                    governance_topics: topics,
                    topic_confidence: confidence,
                    rationale: format!("Agent 请求未知动作 '{}'", action_type),
                }
            }
            GovernanceCode::ExpressionGap => {
                let ir = ProposedActionIR {
                    actor_arity: 1,
                    target_arity: "zero_to_many".into(),
                    tick_span: 0,
                    phase_count: 1,
                    protocol_kind: "none".into(),
                    state_transition_count: 1,
                    effect_refs: vec![action_type.to_string()],
                    requirement_refs: vec![],
                };
                let topics = infer_topics(action_type);
                let confidence: HashMap<GovernanceTopic, f64> =
                    topics.iter().map(|t| (*t, 0.7)).collect();
                SelfEvaluatorOutput {
                    decision: EvaluatorDecision::Propose,
                    ir: Some(ir),
                    governance_topics: topics,
                    topic_confidence: confidence,
                    rationale: format!("Agent 请求动作 '{}'，当前动作表达力不足", action_type),
                }
            }
            GovernanceCode::NonGovernanceReject => SelfEvaluatorOutput {
                decision: EvaluatorDecision::Drop,
                ir: None,
                governance_topics: vec![],
                topic_confidence: HashMap::new(),
                rationale: "普通拒绝，非演化需求".to_string(),
            },
        }
    }
}

fn infer_topics(action_type: &str) -> Vec<GovernanceTopic> {
    let lower = action_type.to_lowercase();
    if lower.contains("combat") || lower.contains("attack") || lower.contains("战斗") {
        return vec![GovernanceTopic::Order];
    }
    if lower.contains("craft") || lower.contains("gather") || lower.contains("制造") {
        return vec![GovernanceTopic::Resource];
    }
    vec![GovernanceTopic::Evolution]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_evaluate_unknown_action() {
        let output = SelfEvaluator::evaluate("mining", GovernanceCode::UnknownAction);
        assert_eq!(output.decision, EvaluatorDecision::Propose);
        assert!(output.ir.is_some());
    }

    #[test]
    fn test_evaluate_expression_gap() {
        let output = SelfEvaluator::evaluate("mining", GovernanceCode::ExpressionGap);
        assert_eq!(output.decision, EvaluatorDecision::Propose);
        assert!(output.ir.is_some());
        assert!(output.rationale.contains("表达力不足"));
    }

    #[test]
    fn test_evaluate_non_governance() {
        let output = SelfEvaluator::evaluate("test", GovernanceCode::NonGovernanceReject);
        assert_eq!(output.decision, EvaluatorDecision::Drop);
    }
}
