use cyber_jianghu_protocol::types::governance::{
    AtomicKind, IRSource, ProposedActionIR, ProtocolKind, TargetArity,
};

use crate::game_data::registry::ActionRegistry;

/// Server 端 IR 生成器
///
/// 在 Agent Intent 被拒绝、且动作未注册时，直接从 `actions.yaml` 或默认值
/// 生成 `ProposedActionIR`，供治理层评审。
///
/// 设计要点（v6 §1.3）：
/// - 已注册 action：从 `ActionConfigEntry` 复制机器可读事实（category）
/// - 未注册 action：标 `IRSource::FromAgentIntent` + `AtomicKind::Unknown`
/// - 第一阶段：仅使用现有 `category` 字段，其他 IR 字段硬编码默认值
/// - 第二阶段（v6 §4.5）：ActionConfigEntry 补全 6 字段后，改用配置字段
pub struct IRGenerator;

impl IRGenerator {
    /// 从已知的 action 注册表生成 IR
    ///
    /// - 注册命中：`IRSource::FromManifest`，从 actions.yaml 复制 category
    /// - 未注册：`IRSource::FromAgentIntent`，全部字段 Unknown/Zero 默认
    pub fn generate(action_type: &str) -> ProposedActionIR {
        if let Some(action) = ActionRegistry::get(action_type) {
            ProposedActionIR {
                source: IRSource::FromManifest,
                atomic_kind: AtomicKind::Atomic,
                actor_arity: 1,
                target_arity: TargetArity::Zero,
                tick_span: 0,
                phase_count: 1,
                protocol_kind: ProtocolKind::None,
                effect_refs: vec![action.category],
                requirement_refs: vec![],
            }
        } else {
            ProposedActionIR {
                source: IRSource::FromAgentIntent,
                atomic_kind: AtomicKind::Unknown,
                actor_arity: 1,
                target_arity: TargetArity::Zero,
                tick_span: 0,
                phase_count: 1,
                protocol_kind: ProtocolKind::None,
                effect_refs: vec![],
                requirement_refs: vec![],
            }
        }
    }
}
