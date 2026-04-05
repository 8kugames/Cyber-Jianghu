// ============================================================================
// OpenClaw Cyber-Jianghu MVP Intent Collector
// ============================================================================
//
// 意图收集器负责从IntentManager收集所有Agent上报的意图，包括：
// 1. 从WebSocket IntentManager缓存中读取所有意图
// 2. 返回意图列表供后续处理
//
// 设计原则：
// 1. 简单的数据收集，不进行任何处理
// 2. 详细日志，方便调试
// 3. 无状态，每次调用独立
// ============================================================================

use anyhow::Result;
use tracing::debug;

use crate::models::{AgentState, Intent};
use crate::websocket::IntentManager;

/// 意图收集器
///
/// 负责从IntentManager收集所有Agent上报的意图
pub struct IntentCollector;

impl IntentCollector {
    /// 创建新的意图收集器
    pub fn new() -> Self {
        Self
    }

    /// 从IntentManager收集所有Agent上报的意图
    ///
    /// 从WebSocket IntentManager缓存中读取当前 tick 的意图
    /// 同时保留未来 tick 的意图供后续使用
    /// 为超时的 Agent 生成默认 idle 意图
    pub async fn collect_intents(
        &self,
        intent_manager: &IntentManager,
        tick_id: i64,
        agent_states: &[AgentState],
    ) -> Result<Vec<Intent>> {
        // 从IntentManager读取当前 tick 的意图
        // 保留未来 tick 的意图供后续使用
        let mut intents = crate::websocket::take_intents_for_tick(intent_manager, tick_id).await;

        // 为未提交意图的存活 Agent 生成默认 idle 意图
        let submitted_agent_ids: std::collections::HashSet<_> =
            intents.iter().map(|i| i.agent_id).collect();

        for state in agent_states {
            if state.is_alive && !submitted_agent_ids.contains(&state.agent_id) {
                debug!(
                    "Agent {} 未提交意图，生成默认 idle 意图 (tick_id: {})",
                    state.agent_id, tick_id
                );
                intents.push(Intent {
                    intent_id: uuid::Uuid::new_v4(),
                    agent_id: state.agent_id,
                    tick_id,
                    thought_log: None,
                    action_type: "idle".into(),
                    action_data: None,
                    priority: 5,
                    observer_thought: None,
                    narrative: Some("静待时机".to_string()),
                    already_broadcast: false,
                    session_id: None,
                });
            }
        }

        // 按优先级降序排序，高优先级先执行
        // 如果优先级相同，可以考虑保持某种确定性，例如按 agent_id
        intents.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.agent_id.cmp(&b.agent_id))
        });

        debug!(
            "从IntentManager收集到 {} 个意图 (tick_id: {})",
            intents.len(),
            tick_id
        );

        // 返回意图列表
        Ok(intents)
    }
}

impl Default for IntentCollector {
    fn default() -> Self {
        Self::new()
    }
}
