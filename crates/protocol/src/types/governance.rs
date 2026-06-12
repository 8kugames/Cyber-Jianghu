use serde::{Deserialize, Serialize};

/// protocol_kind 常量：无协议编排（原子行为）
pub const PROTOCOL_KIND_NONE: &str = "none";

/// 治理议题类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GovernanceTopic {
    Evolution,
    Resource,
    Order,
}

impl GovernanceTopic {
    /// 议题优先级：evolution(0) > resource(1) > order(2)
    pub const fn priority(&self) -> u8 {
        match self {
            Self::Evolution => 0,
            Self::Resource => 1,
            Self::Order => 2,
        }
    }
}

/// 治理分类码（Server 端映射后传递给 Agent）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GovernanceCode {
    /// 未知动作类型
    UnknownAction,
    /// 动作表达力不足
    ExpressionGap,
    /// 普通拒绝（不触发治理）
    NonGovernanceReject,
}

/// 提案 IR（Agent + Server 共享）
///
/// 描述行为的执行特征，用于原子行为判定和治理分类。
/// `target_arity` 使用 String 而非 u8，因为它表示目标数量范围
/// （如 "zero_to_many"、"one"、"many"），而非简单计数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedActionIR {
    /// 发起者数量（1 = 单一发起者，原子行为必须为 1）
    pub actor_arity: u8,
    /// 目标数量范围（"zero_to_many" | "one" | "many"）
    pub target_arity: String,
    /// 跨 tick 结算跨度（0 = 单 tick 结算，原子行为必须为 0）
    pub tick_span: u8,
    /// 阶段数（1 = 无多阶段协议，原子行为必须为 1）
    pub phase_count: u8,
    /// 协议编排类型（"none" | "two_party" | "multi_party" | "staged"）
    pub protocol_kind: String,
    /// 状态转换次数
    pub state_transition_count: u8,
    /// 效果引用（如 "combat.slash"、"mining.dig"）
    pub effect_refs: Vec<String>,
    /// 前置条件引用（如 "tool.pickaxe"）
    pub requirement_refs: Vec<String>,
}

impl ProposedActionIR {
    /// 原子行为判定：单一发起者 + 单 tick 结算 + 无多阶段 + 无协议编排
    pub fn is_atomic(&self) -> bool {
        self.actor_arity == 1
            && self.tick_span == 0
            && self.phase_count == 1
            && self.protocol_kind == PROTOCOL_KIND_NONE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proposed_action_ir_is_atomic() {
        let ir = ProposedActionIR {
            actor_arity: 1,
            target_arity: "zero_to_many".into(),
            tick_span: 0,
            phase_count: 1,
            protocol_kind: PROTOCOL_KIND_NONE.into(),
            state_transition_count: 1,
            effect_refs: vec![],
            requirement_refs: vec![],
        };
        assert!(ir.is_atomic());
    }

    #[test]
    fn test_proposed_action_ir_not_atomic_composite() {
        let ir = ProposedActionIR {
            actor_arity: 2,
            target_arity: "one".into(),
            tick_span: 1,
            phase_count: 2,
            protocol_kind: "two_party".into(),
            state_transition_count: 3,
            effect_refs: vec![],
            requirement_refs: vec![],
        };
        assert!(!ir.is_atomic());
    }

    #[test]
    fn test_governance_topic_priority() {
        assert_eq!(GovernanceTopic::Evolution.priority(), 0);
        assert_eq!(GovernanceTopic::Resource.priority(), 1);
        assert_eq!(GovernanceTopic::Order.priority(), 2);
    }

    #[test]
    fn test_governance_code_serialization() {
        let code = GovernanceCode::UnknownAction;
        let json = serde_json::to_string(&code).unwrap();
        assert_eq!(json, "\"unknown_action\"");
    }
}
