// ============================================================================
// Cognitive Decision - 多阶段认知引擎决策
// ============================================================================
//
// 使用内置的 LLM 进行多阶段认知决策：
// 1. 感知 (Perception) - 理解世界状态
// 2. 动机 (Motivation) - 生成行动动机
// 3. 规划 (Planning) - 制定行动计划
// 4. 决策 (Decision) - 选择最佳意图
// 5. 验证 (Validation) - 验证意图合法性

use crate::soul::actor::CognitiveEngine;
use cyber_jianghu_protocol::{Intent, WorldState};
use futures_util::future::BoxFuture;
use std::sync::Arc;
use tracing::error;
use uuid::Uuid;

/// Cognitive 决策配置
pub struct CognitiveDecisionConfig {
    /// 最大重试次数
    pub max_retries: usize,
}

impl Default for CognitiveDecisionConfig {
    fn default() -> Self {
        Self { max_retries: 3 }
    }
}

/// 创建认知决策函数
///
/// 使用多阶段认知引擎进行决策
pub fn cognitive_decision(
    agent_id: Uuid,
    engine: Arc<CognitiveEngine>,
    _config: CognitiveDecisionConfig,
) -> impl Fn(&WorldState) -> BoxFuture<'static, Intent> + Send + Sync + 'static {
    move |world_state: &WorldState| {
        let engine = engine.clone();
        let world_state = world_state.clone();

        Box::pin(async move {
            // 运行认知流程
            match engine.think(&world_state).await {
                Ok(chain) => chain.final_intent,
                Err(e) => {
                    error!("[cognitive] Decision failed: {}", e);
                    Intent::new(agent_id, world_state.tick_id, "idle", None)
                        .with_thought(format!("认知失败: {}", e))
                }
            }
        })
    }
}

pub fn cognitive_decision_with_retry(
    agent_id: Uuid,
    engine: Arc<CognitiveEngine>,
    max_retries: usize,
) -> impl Fn(&WorldState, Option<&str>) -> BoxFuture<'static, Intent> + Send + Sync + 'static {
    move |world_state: &WorldState, feedback: Option<&str>| {
        let engine = engine.clone();
        let world_state = world_state.clone();
        let feedback = feedback.map(|s| s.to_string());

        Box::pin(async move {
            let mut last_error = String::new();

            for attempt in 0..=max_retries {
                match engine
                    .think_with_memory_and_feedback(&world_state, "", feedback.as_deref())
                    .await
                {
                    Ok(chain) => return chain.final_intent,
                    Err(e) => {
                        last_error = e.to_string();
                        error!("[cognitive] Attempt {} failed: {}", attempt + 1, e);
                    }
                }
            }

            Intent::new(agent_id, world_state.tick_id, "idle", None)
                .with_thought(format!("认知失败({}次重试): {}", max_retries, last_error))
        })
    }
}
