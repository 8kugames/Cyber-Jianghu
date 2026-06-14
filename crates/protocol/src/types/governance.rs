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

/// 原子行为类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AtomicKind {
    Atomic,
    Bilateral,
    MultiPhase,
    Composite,
    #[default]
    Unknown,
}

impl AtomicKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Atomic => "atomic",
            Self::Bilateral => "bilateral",
            Self::MultiPhase => "multi_phase",
            Self::Composite => "composite",
            Self::Unknown => "unknown",
        }
    }
}

/// 目标数量范围
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TargetArity {
    Zero,
    #[default]
    One,
    Many,
}

impl TargetArity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Zero => "zero",
            Self::One => "one",
            Self::Many => "many",
        }
    }
}

/// 协议编排类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolKind {
    #[default]
    None,
    Bilateral,
    MultiPhase,
    Composite,
    Unknown,
}

impl ProtocolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Bilateral => "bilateral",
            Self::MultiPhase => "multi_phase",
            Self::Composite => "composite",
            Self::Unknown => "unknown",
        }
    }
}

/// IR 来源
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IRSource {
    FromManifest,
    FromAgentIntent,
}

/// 提案 IR（Agent + Server 共享）
///
/// 描述行为的执行特征，用于原子行为判定和治理分类。
/// 闸门职责已迁出 IR，原子性判定由 Server 端 Capability Manifest 承担。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedActionIR {
    pub source: IRSource,
    pub atomic_kind: AtomicKind,
    pub actor_arity: u8,
    pub target_arity: TargetArity,
    pub tick_span: u8,
    pub phase_count: u8,
    pub protocol_kind: ProtocolKind,
    pub effect_refs: Vec<String>,
    pub requirement_refs: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proposed_action_ir_atomic_serialization() {
        let ir = ProposedActionIR {
            source: IRSource::FromAgentIntent,
            atomic_kind: AtomicKind::Atomic,
            actor_arity: 1,
            target_arity: TargetArity::One,
            tick_span: 0,
            phase_count: 1,
            protocol_kind: ProtocolKind::None,
            effect_refs: vec![],
            requirement_refs: vec![],
        };
        let json = serde_json::to_string(&ir).unwrap();
        assert!(json.contains("\"source\":\"from_agent_intent\""));
        assert!(json.contains("\"atomic_kind\":\"atomic\""));
        assert!(json.contains("\"target_arity\":\"one\""));
        assert!(json.contains("\"protocol_kind\":\"none\""));
    }

    #[test]
    fn test_proposed_action_ir_composite_serialization() {
        let ir = ProposedActionIR {
            source: IRSource::FromManifest,
            atomic_kind: AtomicKind::Composite,
            actor_arity: 2,
            target_arity: TargetArity::Many,
            tick_span: 1,
            phase_count: 2,
            protocol_kind: ProtocolKind::Bilateral,
            effect_refs: vec![],
            requirement_refs: vec![],
        };
        let json = serde_json::to_string(&ir).unwrap();
        assert!(json.contains("\"source\":\"from_manifest\""));
        assert!(json.contains("\"atomic_kind\":\"composite\""));
        assert!(json.contains("\"target_arity\":\"many\""));
        assert!(json.contains("\"protocol_kind\":\"bilateral\""));
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

    #[test]
    fn test_atomic_kind_as_str() {
        assert_eq!(AtomicKind::Atomic.as_str(), "atomic");
        assert_eq!(AtomicKind::Bilateral.as_str(), "bilateral");
        assert_eq!(AtomicKind::MultiPhase.as_str(), "multi_phase");
        assert_eq!(AtomicKind::Composite.as_str(), "composite");
        assert_eq!(AtomicKind::Unknown.as_str(), "unknown");
    }

    #[test]
    fn test_target_arity_as_str() {
        assert_eq!(TargetArity::Zero.as_str(), "zero");
        assert_eq!(TargetArity::One.as_str(), "one");
        assert_eq!(TargetArity::Many.as_str(), "many");
    }

    #[test]
    fn test_protocol_kind_as_str() {
        assert_eq!(ProtocolKind::None.as_str(), "none");
        assert_eq!(ProtocolKind::Bilateral.as_str(), "bilateral");
        assert_eq!(ProtocolKind::MultiPhase.as_str(), "multi_phase");
        assert_eq!(ProtocolKind::Composite.as_str(), "composite");
        assert_eq!(ProtocolKind::Unknown.as_str(), "unknown");
    }
}
