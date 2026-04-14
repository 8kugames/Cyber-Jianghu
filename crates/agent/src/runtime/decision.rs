// ============================================================================
// Cognitive Decision - 认知引擎决策
// ============================================================================
//
// 5 阶段认知管线（非线性管道）：
//   1. 感知 (Perception)   ─┐
//   2. 动机 (Motivation)   ─┘ LLM Call 1（合并）
//   3. 规划 (Planning)     ─┐
//   4. 决策 (Decision)     ─┘ LLM Call 2（合并）
//   5. 验证 (Validation)
//      5a. CognitiveValidator 认知链质量审查（本文件，重试循环内）
//      5b. ReflectorSoul 规则/世界观审查（lifecycle.rs，外部）

use crate::soul::actor::{CognitiveChain, CognitiveEngine};
use crate::soul::reflector::cognitive_validator::CognitiveValidator;
use cyber_jianghu_protocol::Intent;
use futures_util::future::BoxFuture;
use std::sync::Arc;
use tracing::{error, warn};
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
/// 使用认知引擎进行决策（5 阶段管线，2 次合并 LLM 调用）
pub fn cognitive_decision(
    engine: Arc<CognitiveEngine>,
    _config: CognitiveDecisionConfig,
) -> impl Fn(i64, Uuid) -> BoxFuture<'static, Intent> + Send + Sync + 'static {
    move |tick_id: i64, agent_id: Uuid| {
        let engine = engine.clone();

        Box::pin(async move {
            // 运行认知流程
            match engine.think(tick_id, agent_id).await {
                Ok(chain) => chain.final_intent,
                Err(e) => {
                    error!("[cognitive] Decision failed: {}", e);
                    Intent::new(agent_id, tick_id, "idle", None)
                        .with_thought(format!("认知失败: {}", e))
                }
            }
        })
    }
}

/// 创建带 CognitiveChain 返回的认知决策函数
///
/// 使用认知引擎进行决策，返回 (Intent, Option<CognitiveChain>) 元组。
/// CognitiveChain 供天魂翻译时获取认知上下文辅助指代消解。
#[allow(clippy::type_complexity)]
pub fn cognitive_decision_with_chain(
    engine: Arc<CognitiveEngine>,
    max_retries: usize,
) -> impl Fn(i64, Uuid, &str, Option<&str>) -> BoxFuture<'static, (Intent, Option<CognitiveChain>)>
+ Send
+ Sync
+ 'static {
    move |tick_id: i64, agent_id: Uuid, memory_context: &str, feedback: Option<&str>| {
        let engine = engine.clone();
        let memory_context = memory_context.to_string();
        let feedback = feedback.map(|s| s.to_string());

        Box::pin(async move {
            let mut last_error = String::new();
            let mut last_chain: Option<CognitiveChain> = None;

            for attempt in 0..=max_retries {
                match engine
                    .think_unified(tick_id, agent_id, &memory_context, feedback.as_deref())
                    .await
                {
                    Ok(chain) => {
                        let final_intent = chain.final_intent.clone();
                        last_chain = Some(chain.clone());
                        // CognitiveValidator: 验证认知链质量
                        let validator = CognitiveValidator::new(chain.persona.clone());
                        let validation = validator.validate(&chain);
                        if validation.is_valid {
                            return (final_intent, Some(chain));
                        }

                        let reason = validation.reason.unwrap_or_default();
                        let suggestion = validation.suggestion.unwrap_or_default();
                        warn!(
                            "[cognitive] Validator rejected (attempt {}/{}): {} | suggestion: {}",
                            attempt + 1,
                            max_retries + 1,
                            reason,
                            suggestion
                        );

                        if attempt == max_retries {
                            warn!(
                                "[cognitive] Max retries reached, using intent despite validation failure"
                            );
                            return (final_intent, Some(chain));
                        }
                    }
                    Err(e) => {
                        last_error = e.to_string();
                        error!("[cognitive] Attempt {} failed: {}", attempt + 1, e);
                    }
                }
            }

            let idle_intent = Intent::new(agent_id, tick_id, "idle", None)
                .with_thought(format!("认知失败({}次重试): {}", max_retries, last_error));
            (idle_intent, last_chain)
        })
    }
}
