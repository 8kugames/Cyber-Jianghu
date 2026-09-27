// ============================================================================
// LLM 消费场景标签（task-local 传播 + token 记账维度）
// ============================================================================
//
// 目的：回答「单日 token 消耗花在哪个环节」。调用点用 `with_scenario` 包住
// LLM 调用，记账层（http.rs / streaming.rs）在 record_token_usage 时读
// task-local 场景，落盘到 token_cost_count.tmp 的 by_scenario 维度。
//
// 同时驱动场景级模型路由：DirectLlmClient 构建请求时按当前场景查
// `scenario_overrides`，辅助任务（审查/摘要/叙事）可路由到更便宜的模型。
//
// 传播机制选 task-local 而非方法签名透传的原因：
// - LlmClientExt 的 complete_json* 族方法签名已被 10+ 调用点使用，
//   逐层加参会污染所有中间层（fallback / streaming / tool_loop）；
// - task-local 在同一 tokio task 内自动贯通 await 链，fallback 切换客户端
//   不影响标签；嵌套 scope 内层覆盖外层（tool loop 轮次覆盖 think 外层）。

use std::sync::atomic::{AtomicU64, Ordering};

tokio::task_local! {
    static LLM_SCENARIO: Scenario;
}

/// 场景标签（Copy，零分配）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Scenario(pub &'static str);

/// 主认知决策（ActorSoul 非工具路径）
pub const THINK: Scenario = Scenario("think");
/// 地魂 tool loop 每轮 LLM 调用（含强制文本退出）
pub const THINK_TOOL_ROUND: Scenario = Scenario("think_tool_round");
/// 天魂 Layer 3 LLM 审查
pub const REFLECTOR_L3: Scenario = Scenario("reflector_l3");
/// 每游戏日 triage 批量分诊
pub const SESSION_TRIAGE: Scenario = Scenario("session_triage");
/// 每游戏日日记/纪要生成
pub const DAILY_SUMMARY: Scenario = Scenario("daily_summary");
/// 记忆叙事合成（NarrativeEngine）
pub const NARRATIVE: Scenario = Scenario("narrative");
/// 对话历史压缩摘要（compaction）
pub const CONVERSATION_SUMMARY: Scenario = Scenario("conversation_summary");
/// 关系主观评估（social.rs）
pub const RELATIONSHIP_EVAL: Scenario = Scenario("relationship_eval");
/// 好感度描述生成
pub const RELATIONSHIP_NARRATIVE: Scenario = Scenario("relationship_narrative");
/// 传记生成
pub const BIOGRAPHY: Scenario = Scenario("biography");
/// 角色注册一键生成
pub const CHARACTER_GENERATION: Scenario = Scenario("character_generation");
/// 未标记场景（调用点未包裹时兜底）
pub const UNKNOWN: Scenario = Scenario("unknown");

/// 在指定场景标签下执行 future（嵌套时内层覆盖外层）
pub async fn with_scenario<R>(scenario: Scenario, fut: impl std::future::Future<Output = R>) -> R {
    LLM_SCENARIO.scope(scenario, fut).await
}

/// 读取当前场景标签；无标签（调用点未包裹）返回 UNKNOWN
pub fn current() -> Scenario {
    LLM_SCENARIO.try_with(|s| *s).unwrap_or(UNKNOWN)
}

// ============================================================================
// Tool loop 轮次计数（轮次边际价值的观测数据）
// ============================================================================

/// 轮次计数桶数：覆盖 round 0..=14，超过 14 轮计入最后一桶
const TOOL_ROUND_BUCKETS: usize = 15;

static TOOL_ROUND_CALLS: [AtomicU64; TOOL_ROUND_BUCKETS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; TOOL_ROUND_BUCKETS]
};

/// 强制文本退出次数（max_rounds 耗尽 / budget 耗尽 / loop guard 截断）
static FORCED_TEXT_EXITS: AtomicU64 = AtomicU64::new(0);

/// 记录一次 tool loop 轮次调用（round 从 0 计）
pub fn record_tool_round(round: usize) {
    let idx = round.min(TOOL_ROUND_BUCKETS - 1);
    TOOL_ROUND_CALLS[idx].fetch_add(1, Ordering::Relaxed);
}

/// 记录一次强制文本退出
pub fn record_forced_text_exit() {
    FORCED_TEXT_EXITS.fetch_add(1, Ordering::Relaxed);
}

/// 轮次快照：`Vec<(round, calls)>`（round=14 桶语义为「>=14」）
pub fn snapshot_tool_rounds() -> Vec<(usize, u64)> {
    (0..TOOL_ROUND_BUCKETS)
        .map(|i| (i, TOOL_ROUND_CALLS[i].load(Ordering::Relaxed)))
        .filter(|&(_, calls)| calls > 0)
        .collect()
}

/// 强制文本退出次数快照
pub fn snapshot_forced_text_exits() -> u64 {
    FORCED_TEXT_EXITS.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_with_scenario_sets_and_restores() {
        assert_eq!(current(), UNKNOWN);
        let inner = with_scenario(THINK, async {
            assert_eq!(current(), THINK);
            // 嵌套覆盖
            let nested = with_scenario(THINK_TOOL_ROUND, async {
                assert_eq!(current(), THINK_TOOL_ROUND);
            });
            nested.await;
            assert_eq!(current(), THINK);
        });
        inner.await;
        assert_eq!(current(), UNKNOWN);
    }

    #[test]
    fn test_tool_round_counters() {
        record_tool_round(0);
        record_tool_round(0);
        record_tool_round(2);
        record_tool_round(99); // 溢出桶
        let snap = snapshot_tool_rounds();
        assert!(snap.contains(&(0, 2)) || snap.iter().any(|&(r, c)| r == 0 && c >= 2));
        assert!(
            snap.iter().any(|&(r, c)| r == 14 && c >= 1),
            "round 99 应计入溢出桶"
        );
    }
}
