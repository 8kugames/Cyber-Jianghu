use cyber_jianghu_protocol::types::governance::{
    AtomicKind, IRSource, ProposedActionIR, ProtocolKind, TargetArity,
};

use crate::game_data::registry::ActionRegistry;

/// Server 端 IR 生成器
///
/// 在 Agent Intent 被拒绝、且动作未注册时，从 `actions.yaml` 或默认值
/// 生成 `ProposedActionIR`，供治理层评审。
///
/// # Phase 0 简化（仅伏羲上线时）
///
/// 当前 SoulsConfig 仅启用 fuxi（evolution 角色）。伏羲审议关注"动作是否
/// 促进世界多样性"，不消费执行特征字段。因此本生成器对 Phase 0 做以下简化：
///
/// - `effect_refs = vec![action.category]`：effect_refs 设计语义是"动作效果引用"
///   （如 "combat.slash"），但伏羲 classifier 只用 effect_refs 做 topic 路由
///   （prefix 匹配），用 category（如 "combat"）作 prefix 等价。Phase 2 神农/
///   轩辕上线后需要真实 effect_refs，应改为从 ActionConfigEntry.effects 派生。
///
/// - `AtomicKind::Atomic`（已注册）硬编码：伏羲不关心原子性。Phase 2 轩辕
///   （order 角色）上线后需要真实判定，应改为基于 actor_arity/tick_span/
///   phase_count/protocol_kind 的纯函数计算（替代已删除的 is_atomic()）。
///
/// - 其他执行特征字段（actor_arity/target_arity/tick_span/phase_count/
///   protocol_kind）硬编码默认值：Phase 2 全部 souls 上线后，应从
///   ActionConfigEntry 的对应字段（v6 §4.5 已补）派生。
///
/// 设计要点（v6 §1.3）：
/// - 已注册 action：`IRSource::FromManifest`
/// - 未注册 action：`IRSource::FromAgentIntent` + 全部字段 Unknown/Zero 默认
pub struct IRGenerator;

impl IRGenerator {
    /// 从已知的 action 注册表生成 IR
    ///
    /// - 注册命中：`IRSource::FromManifest`，从 actions.yaml 复制 category
    /// - 未注册：`IRSource::FromAgentIntent`，全部字段 Unknown/Zero 默认
    ///
    /// 注：Phase 0 字段语义简化见 struct 文档注释
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
