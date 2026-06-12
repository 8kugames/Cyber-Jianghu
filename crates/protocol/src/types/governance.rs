use serde::{Deserialize, Serialize};

/// 治理议题类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GovernanceTopic {
    Evolution,
    Resource,
    Order,
}

impl GovernanceTopic {
    pub fn priority(&self) -> u8 {
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposedActionIR {
    pub actor_arity: u8,
    pub target_arity: String,
    pub tick_span: u8,
    pub phase_count: u8,
    pub protocol_kind: String,
    pub state_transition_count: u8,
    pub effect_refs: Vec<String>,
    pub requirement_refs: Vec<String>,
}

impl ProposedActionIR {
    pub fn is_atomic(&self) -> bool {
        self.actor_arity == 1
            && self.tick_span == 0
            && self.phase_count == 1
            && self.protocol_kind == "none"
    }
}
