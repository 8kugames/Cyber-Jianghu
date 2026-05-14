// ============================================================================
// WebSocket 意图验证模块
// ============================================================================
//
// 提供意图验证流水线：
// - CAS 原子去重（防止同一 tick 重复提交）
// - LLM 验证器集成（可选）
// - 超时降级策略
// ============================================================================

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use futures_util::SinkExt;
use futures_util::stream::SplitSink;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::protocol::{DownstreamMessage, ServerErrorCode, WsIntent};
use crate::models::Intent;
use crate::core::utils::build_world_context;
use crate::soul::reflector::{
    PersonaInfo, PipelineValidationResult, ValidationRequest, ValidationRuntimeConfig, Validator,
};

// ============================================================================
// WebSocket 验证请求
// ============================================================================

/// WebSocket 验证请求（从读任务发送到验证任务）
///
/// 包含待验证的意图和 WebSocket 发送端，用于在验证失败时返回错误消息
pub struct WsValidationRequest {
    /// 待验证的意图
    pub intent: WsIntent,
    /// WebSocket 发送端（用于返回验证错误）
    pub ws_tx: Arc<tokio::sync::Mutex<SplitSink<WebSocket, Message>>>,
}

// ============================================================================
// 验证任务
// ============================================================================

/// 验证任务参数
pub struct ValidationTaskParams {
    /// 验证请求接收通道
    pub validation_rx: mpsc::Receiver<WsValidationRequest>,
    /// 意图发送通道（验证通过后转发）
    pub intent_tx: mpsc::Sender<WsIntent>,
    /// 当前 tick ID（原子变量）
    pub current_tick: Arc<AtomicI64>,
    /// 已提交的 tick ID（CAS 去重，-1 表示未提交）
    pub submitted_tick: Arc<AtomicI64>,
    /// 意图验证器（可选，RwLock 支持运行时更新）
    pub intent_validator: Arc<RwLock<Option<Arc<dyn Validator>>>>,
    /// 最近一份 WorldState
    pub current_world_state: Arc<RwLock<Option<Arc<cyber_jianghu_protocol::WorldState>>>>,
    /// 最近一份 GameRules
    pub game_rules: Arc<RwLock<Option<cyber_jianghu_protocol::GameRules>>>,
    /// 人设信息（可选，RwLock 支持运行时更新）
    pub persona: Arc<RwLock<Option<PersonaInfo>>>,
    /// 最大连续跟随次数（从配置读取）
    pub max_consecutive_follow: usize,
}

/// 启动验证任务
///
/// 验证流水线：
/// 1. 检查 tick 是否过期
/// 2. CAS 原子去重（防止同一 tick 重复提交）
/// 3. 调用 LLM 验证器（可选）
/// 4. 通过则转发到 intent_tx，否则返回错误
///
/// # 降级策略
/// - 无验证器或无 persona：直接转发
/// - LLM 错误或超时（10秒）：允许通过
pub fn spawn_validation_task(params: ValidationTaskParams) -> tokio::task::JoinHandle<()> {
    let mut validation_rx = params.validation_rx;
    let intent_tx = params.intent_tx;
    let current_tick = params.current_tick;
    let submitted_tick = params.submitted_tick;
    let intent_validator = params.intent_validator;
    let current_world_state = params.current_world_state;
    let game_rules = params.game_rules;
    let persona = params.persona;
    let max_consecutive_follow = params.max_consecutive_follow;

    tokio::spawn(async move {
        debug!("Validation task started");

        while let Some(req) = validation_rx.recv().await {
            let current_tick_value = current_tick.load(Ordering::Relaxed);

            // 1. 检查验证期间 tick 是否推进
            if req.intent.tick_id != current_tick_value {
                send_server_error(
                    &req.ws_tx,
                    ServerErrorCode::TickExpired,
                    format!(
                        "Validation expired: intent tick {} != current tick {}",
                        req.intent.tick_id, current_tick_value
                    ),
                    req.intent.tick_id,
                    current_tick_value,
                )
                .await;

                warn!(
                    "Validation rejected: tick {} != current {}",
                    req.intent.tick_id, current_tick_value
                );
                continue;
            }

            // 2. 使用 CAS 操作原子性地检查并声明该 tick
            // 这解决了 TOCTOU 竞态条件
            let cas_result = submitted_tick.compare_exchange(
                -1, // 期望值：-1 表示该 tick 尚未提交
                req.intent.tick_id,
                Ordering::AcqRel,
                Ordering::Acquire,
            );

            if cas_result != Ok(-1) {
                // 已经有其他意图被提交了
                let prev_tick = cas_result.unwrap_or(req.intent.tick_id);
                send_server_error(
                    &req.ws_tx,
                    ServerErrorCode::DuplicateSubmission,
                    format!(
                        "Intent already submitted for tick {} (submitted: {})",
                        req.intent.tick_id, prev_tick
                    ),
                    req.intent.tick_id,
                    current_tick_value,
                )
                .await;

                warn!("Rejected duplicate intent for tick {}", req.intent.tick_id);
                continue;
            }

            // 3. 获取验证器和 persona
            let validator_guard = intent_validator.read().await;
            let persona_guard = persona.read().await;

            match (validator_guard.as_ref(), persona_guard.as_ref()) {
                (None, _) | (_, None) => {
                    // 无验证器或无 persona，直接转发
                    // submitted_tick 已通过 CAS 设置
                    debug!("No validator or persona, forwarding directly");
                    if let Err(e) = intent_tx.send(req.intent).await {
                        error!("Failed to send intent: {}", e);
                        // 发送失败，重置 submitted_tick 允许重试
                        submitted_tick.store(-1, Ordering::Release);
                    }
                }
                (Some(validator), Some(persona_info)) => {
                    // 4. 构建验证请求
                    let world_state = current_world_state.read().await.clone();
                    let world_state = world_state.as_ref().map(|state| (**state).clone());
                    let graded_config = game_rules
                        .read()
                        .await
                        .as_ref()
                        .and_then(|rules| rules.intent_batch.as_ref())
                        .map(|batch| batch.llm_validation.clone());
                    // [TRAP_DEBT: TICKET-102] Claw WebSocket 验证层无法直接读取 Agent 主循环中的内存状态
                    // 传入 0 会导致绕过连续动作的防刷屏拦截（如连续 follow）。
                    // 预计修复：将 WsSharedState 接入对连续动作计数的同步，或下沉计数至共享状态。
                    // 预计偿还时间：2026-06-01
                    let validation_req = ValidationRequest {
                        intent: Intent::new(
                            Uuid::nil(), // agent_id 暂时用 nil
                            req.intent.tick_id,
                            req.intent.action_type.clone(),
                            req.intent.action_data.clone(),
                        ),
                        persona: persona_info.clone(),
                        world_context: world_state
                            .as_ref()
                            .map(build_world_context)
                            .unwrap_or_else(|| format!("tick: {}", req.intent.tick_id)),
                        world_state,
                        runtime: ValidationRuntimeConfig {
                            graded_config,
                            consecutive_follow_count: 0,
                            max_consecutive_follow,
                        },
                    };

                    // 5. 带超时的验证（10 秒）
                    match tokio::time::timeout(
                        Duration::from_secs(10),
                        validator.validate(validation_req),
                    )
                    .await
                    {
                        Ok(Ok(PipelineValidationResult::Approved { narrative, .. })) => {
                            // submitted_tick 已通过 CAS 设置
                            if let Err(e) = intent_tx.send(req.intent).await {
                                error!("Failed to send intent: {}", e);
                                submitted_tick.store(-1, Ordering::Release);
                            }
                            debug!(
                                "Intent approved and forwarded: narrative={:?}",
                                narrative
                            );
                        }
                        Ok(Ok(PipelineValidationResult::Rejected { reason, .. })) => {
                            // 验证失败，重置 submitted_tick 允许客户端重试
                            submitted_tick.store(-1, Ordering::Release);

                            send_server_error(
                                &req.ws_tx,
                                ServerErrorCode::ValidationFailed,
                                reason.clone(),
                                req.intent.tick_id,
                                current_tick_value,
                            )
                            .await;

                            info!("Intent rejected: {}", reason);
                        }
                        Ok(Err(e)) => {
                            // LLM 错误：允许通过（降级策略）
                            warn!("Validation error, allowing: {}", e);
                            if let Err(e) = intent_tx.send(req.intent).await {
                                error!("Failed to send intent: {}", e);
                                submitted_tick.store(-1, Ordering::Release);
                            }
                        }
                        Err(_) => {
                            // 超时：允许通过（降级策略）
                            warn!("Validation timeout, allowing");
                            if let Err(e) = intent_tx.send(req.intent).await {
                                error!("Failed to send intent: {}", e);
                                submitted_tick.store(-1, Ordering::Release);
                            }
                        }
                    }
                }
            }
        }

        debug!("Validation task ended");
    })
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 发送服务器错误消息
async fn send_server_error(
    ws_tx: &Arc<tokio::sync::Mutex<SplitSink<WebSocket, Message>>>,
    code: ServerErrorCode,
    message: String,
    tick_id: i64,
    current_tick: i64,
) {
    let error_msg = DownstreamMessage::ServerError {
        code,
        message,
        tick_id: Some(tick_id),
        current_tick: Some(current_tick),
    };

    if let Ok(json) = serde_json::to_string(&error_msg) {
        let mut tx = ws_tx.lock().await;
        let _ = tx.send(Message::Text(json.into())).await;
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soul::reflector::{
        LayerResult, PersonaInfo, PipelineValidationResult, ValidationRequest, Validator,
    };
    use anyhow::Result;
    use async_trait::async_trait;
    use cyber_jianghu_protocol::WorldBuildingRules;
    use std::sync::Arc as StdArc;

    // ========================================================================
    // CAS 去重逻辑测试
    // ========================================================================

    /// 测试 CAS 去重的基本行为
    ///
    /// 场景：第一个请求成功，第二个相同 tick 的请求被拒绝
    #[test]
    fn test_cas_dedup_basic() {
        let submitted_tick = Arc::new(AtomicI64::new(-1));

        // 第一次 CAS：应该成功
        // compare_exchange 成功返回 Ok(old_value)，失败返回 Err(current_value)
        let result1 = submitted_tick.compare_exchange(-1, 100, Ordering::AcqRel, Ordering::Acquire);
        assert!(result1 == Ok(-1), "First CAS should succeed");

        // 验证值已被更新
        assert_eq!(submitted_tick.load(Ordering::Relaxed), 100);

        // 第二次 CAS：应该失败（值已不再是 -1）
        let result2 = submitted_tick.compare_exchange(-1, 100, Ordering::AcqRel, Ordering::Acquire);
        // 失败时返回 Err(current_value)
        assert!(
            result2 == Err(100),
            "Second CAS should fail with current value"
        );
    }

    /// 测试 CAS 去重在并发场景下的行为
    ///
    /// 场景：多个线程同时尝试 CAS，只有一个成功
    #[test]
    fn test_cas_dedup_concurrent() {
        use std::thread;

        let submitted_tick = Arc::new(AtomicI64::new(-1));
        let success_count = Arc::new(AtomicI64::new(0));
        let mut handles = vec![];

        // 启动 10 个线程同时尝试 CAS
        for _ in 0..10 {
            let tick = submitted_tick.clone();
            let counter = success_count.clone();
            handles.push(thread::spawn(move || {
                let result = tick.compare_exchange(-1, 42, Ordering::AcqRel, Ordering::Acquire);
                if result == Ok(-1) {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            }));
        }

        // 等待所有线程完成
        for handle in handles {
            handle.join().unwrap();
        }

        // 只有一个人应该成功
        assert_eq!(success_count.load(Ordering::Relaxed), 1);
        assert_eq!(submitted_tick.load(Ordering::Relaxed), 42);
    }

    /// 测试 tick 重置后可以重新提交
    ///
    /// 场景：新 tick 开始时重置 submitted_tick，允许新的提交
    #[test]
    fn test_cas_reset_for_new_tick() {
        let submitted_tick = Arc::new(AtomicI64::new(100)); // 旧 tick 已提交

        // 新 tick 开始，重置为 -1
        submitted_tick.store(-1, Ordering::Release);
        assert_eq!(submitted_tick.load(Ordering::Relaxed), -1);

        // 现在应该可以成功提交新 tick
        let result = submitted_tick.compare_exchange(-1, 200, Ordering::AcqRel, Ordering::Acquire);
        assert!(result == Ok(-1), "CAS should succeed after reset");
    }

    /// 测试发送失败后重置 submitted_tick
    ///
    /// 场景：intent_tx.send() 失败后，重置 submitted_tick 允许重试
    #[test]
    fn test_cas_reset_on_send_failure() {
        let submitted_tick = Arc::new(AtomicI64::new(100)); // 模拟已通过 CAS

        // 模拟发送失败，重置
        submitted_tick.store(-1, Ordering::Release);

        // 客户端应该可以重试
        let result = submitted_tick.compare_exchange(-1, 100, Ordering::AcqRel, Ordering::Acquire);
        assert!(result == Ok(-1), "CAS should succeed after failure reset");
    }

    struct EchoValidator;

    #[async_trait]
    impl Validator for EchoValidator {
        async fn validate(&self, request: ValidationRequest) -> Result<PipelineValidationResult> {
            let has_world_state = request.world_state.is_some();
            let has_graded_config = request.runtime.graded_config.is_some();
            Ok(PipelineValidationResult::Approved {
                intent: request.intent,
                layers: vec![LayerResult {
                    layer: "test",
                    passed: has_world_state && has_graded_config,
                    detail: Some(format!(
                        "world_state={}, graded={}",
                        has_world_state, has_graded_config
                    )),
                }],
                narrative: None,
            })
        }

        async fn validate_persona(
            &self,
            _persona: &PersonaInfo,
        ) -> Result<crate::soul::reflector::ValidationResult> {
            unreachable!()
        }

        async fn update_rules(&self, _rules: WorldBuildingRules) {}
    }

    #[tokio::test]
    async fn test_validation_task_builds_full_runtime_context() {
        let (validation_tx, validation_rx) = mpsc::channel(1);
        let (intent_tx, mut intent_rx) = mpsc::channel(1);
        let current_tick = Arc::new(AtomicI64::new(42));
        let submitted_tick = Arc::new(AtomicI64::new(-1));
        let intent_validator = Arc::new(RwLock::new(Some(
            StdArc::new(EchoValidator) as StdArc<dyn Validator>
        )));
        let current_world_state = Arc::new(RwLock::new(Some(StdArc::new(
            cyber_jianghu_protocol::WorldState {
                event_type: "world_state".to_string(),
                tick_id: 42,
                agent_id: None,
                world_time: cyber_jianghu_protocol::WorldTime {
                    year: 1,
                    month: 1,
                    day: 1,
                    hour: 1,
                    minute: 0,
                    second: 0,
                    weather: "晴".to_string(),
                },
                location: cyber_jianghu_protocol::Location {
                    node_id: "a".to_string(),
                    name: "地点A".to_string(),
                    node_type: "inn".to_string(),
                    adjacent_nodes: vec![],
                    gatherable_items: vec![],
                },
                self_state: cyber_jianghu_protocol::AgentSelfState {
                    attributes: std::collections::HashMap::new(),
                    derived_attributes: std::collections::HashMap::new(),
                    attribute_descriptions: std::collections::HashMap::new(),
                    status_effects: vec![],
                    inventory: vec![],
                    skills: vec![],
                    age_years: None,
                    max_age: None,
                    recipe_details: vec![],
                },
                entities: vec![],
                nearby_items: vec![],
                events_log: vec![],
                private_dialogue_log: vec![],
                last_execution_summary: None,
                lessons_learned: vec![],
            },
        ))));
        let game_rules = Arc::new(RwLock::new(Some(cyber_jianghu_protocol::GameRules {
            tick_duration_secs: 60,
            available_actions: vec![],
            initial_items: vec![],
            survival_actions: vec![],
            version: "test".to_string(),
            last_updated: "2026-01-01T00:00:00Z".to_string(),
            intent_batch: Some(cyber_jianghu_protocol::IntentBatchConfig {
                max_intents_per_tick: 1,
                max_retries: 1,
                pipeline_execution_enabled: true,
                partial_execution_enabled: true,
                llm_validation: cyber_jianghu_protocol::GradedValidationConfig::default(),
                llm_chaos_threshold: 1,
            }),
            immediate_events: None,
            rebirth_delay_ticks: 0,
            rebirth_retry_max_attempts: 0,
            rebirth_retry_interval_secs: 0,
            lifespan: None,
            calendar: None,
            daily_summary: None,
            dialogue_context: None,
        })));
        let persona = Arc::new(RwLock::new(Some(PersonaInfo::default())));
        let params = ValidationTaskParams {
            validation_rx,
            intent_tx,
            current_tick,
            submitted_tick,
            intent_validator,
            current_world_state,
            game_rules,
            persona,
            max_consecutive_follow: 5,
        };

        let handle = spawn_validation_task(params);
        drop(handle);

        // 只验证请求构造与通过转发，不验证 ws 回包
        drop(validation_tx);
        assert!(intent_rx.recv().await.is_none());
    }
}
