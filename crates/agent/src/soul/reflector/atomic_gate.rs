use cyber_jianghu_protocol::ProposedActionIR;
use super::governance::EvaluatorDecision;

pub fn check_atomicity(ir: &ProposedActionIR, decision: &mut EvaluatorDecision) {
    if !ir.is_atomic() {
        *decision = EvaluatorDecision::Drop;
    }
}
