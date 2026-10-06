// ============================================================================
// 观测端点：认知上下文 / 死亡事件 SSE / LLM metrics
// ============================================================================
// 自 llm_config.rs 外移（与 LLM 配置无关注观侧 handler）；路由经 handlers/mod.rs
// glob 再导出保持路径不变。

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use http_body::Frame;
use http_body_util::StreamBody;
use serde::{Deserialize, Serialize};

use super::HttpApiState;
use std::time::Duration;

use super::ErrorResponse;
use super::sse_util::{HEARTBEAT_INTERVAL_SECS, sse_frame, sse_response};
use cyber_jianghu_protocol::ServerMessage;

use crate::infra::api::cognitive_context::{CognitiveContext, CognitiveContextBuilder};

// 认知上下文端点
// ============================================================================

/// 认知端点返回的人设信息（从 DynamicPersona 提取）
#[derive(Debug, Serialize)]
pub struct CognitivePersonaInfo {
    pub name: String,
    pub personality: Vec<String>,
    pub description: String,
}

/// 简化的世界状态（用于认知上下文）
#[derive(Debug, Serialize)]
pub struct SimplifiedWorldState {
    pub agent_id: Option<String>,
    pub attributes: std::collections::HashMap<String, i32>,
    pub nearby_entities_count: usize,
    pub time: SimplifiedTime,
}

/// 简化的时间
#[derive(Debug, Serialize)]
pub struct SimplifiedTime {
    pub hour: i32,
    pub weather: String,
}

/// 认知上下文响应
#[derive(Debug, Serialize)]
pub struct CognitiveContextResponse {
    pub cognitive_context: CognitiveContext,
    pub persona: Option<CognitivePersonaInfo>,
    pub world_state: SimplifiedWorldState,
}

/// GET /api/v1/cognitive - 获取结构化认知上下文
///
/// 返回引导 OpenClaw LLM 进行按阶段推理的结构化上下文
pub(crate) async fn get_cognitive_context_handler(
    State(state): State<HttpApiState>,
) -> impl IntoResponse {
    let current = state.current_state.read().await;

    match current.as_ref() {
        Some(world_state) => {
            let builder = CognitiveContextBuilder::new(Default::default());

            let persona_opt = state
                .dynamic_persona
                .read()
                .expect("rwlock poisoned")
                .clone();
            let (persona_info, persona_ref): (
                Option<CognitivePersonaInfo>,
                Option<crate::component::persona::dynamic_persona::DynamicPersona>,
            ) = if let Some(ref persona_arc) = persona_opt {
                persona_arc.read(|p| {
                    let info = CognitivePersonaInfo {
                        name: p.name.clone(),
                        personality: p.traits.keys().take(3).cloned().collect(),
                        description: p.base_description.clone(),
                    };
                    (Some(info), Some(p.clone()))
                })
            } else {
                (None, None)
            };

            let store_arc = state
                .relationship_store
                .read()
                .expect("rwlock poisoned")
                .clone();
            let relationship_store = store_arc.as_deref();
            let cognitive_context =
                builder.build_with_persona(world_state, persona_ref.as_ref(), relationship_store);

            let simplified_world_state = SimplifiedWorldState {
                agent_id: world_state.agent_id.map(|id| id.to_string()),
                attributes: world_state.self_state.attributes.clone(),
                nearby_entities_count: world_state.entities.len(),
                time: SimplifiedTime {
                    hour: world_state.world_time.hour,
                    weather: world_state.world_time.weather.clone(),
                },
            };

            let response = CognitiveContextResponse {
                cognitive_context,
                persona: persona_info,
                world_state: simplified_world_state,
            };

            (StatusCode::OK, Json(response)).into_response()
        }
        None => {
            let error = ErrorResponse {
                error_code: "NO_WORLD_STATE".to_string(),
                message: "No world state available".to_string(),
            };
            (StatusCode::SERVICE_UNAVAILABLE, Json(error)).into_response()
        }
    }
}

/// GET /api/v1/events - SSE 实时事件流
///
/// 用于 Web 面板实时接收死亡等事件通知
pub(crate) async fn death_events_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let mut death_rx = state.death_event_tx.subscribe();
    let mut tick_rx = state.tick_update_tx.subscribe();

    let stream = async_stream::stream! {
        let data = sse_frame("connected", r#"{"status":"connected"}"#);
        yield Ok::<_, std::convert::Infallible>(Frame::data(data));

        loop {
            tokio::select! {
                death_result = tokio::time::timeout(
                    Duration::from_secs(HEARTBEAT_INTERVAL_SECS),
                    death_rx.recv()
                ) => {
                    match death_result {
                        Ok(Ok(msg)) => {
                            if matches!(msg, ServerMessage::AgentDied { .. })
                                && let Ok(json) = serde_json::to_string(&msg) {
                                let data = sse_frame("agent_died", &json);
                                yield Ok::<_, std::convert::Infallible>(Frame::data(data));
                            }
                        }
                        Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped))) => {
                            // 消费端落后：丢弃的是旧消息，通道仍活，继续跟流（不可断连）
                            tracing::warn!("[events] death broadcast lagged, skipped {} msgs", skipped);
                        }
                        Ok(Err(_)) => {
                            break;
                        }
                        Err(_) => {
                            let data = sse_frame("heartbeat", "{}");
                            yield Ok::<_, std::convert::Infallible>(Frame::data(data));
                        }
                    }
                }
                tick_result = tick_rx.recv() => {
                    match tick_result {
                        Ok(tick_id) => {
                            let json = serde_json::json!({"tick_id": tick_id}).to_string();
                            let data = sse_frame("tick_update", &json);
                            yield Ok::<_, std::convert::Infallible>(Frame::data(data));
                        }
                        // 消费端落后：丢弃的是旧帧，通道仍活，继续跟流（不可断连——
                        // 本流还承载 agent_died；对齐 state_stream 的 Lagged 降级。
                        // tick_update 每 tick 必发后此分支真实可达）
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::warn!("[events] tick broadcast lagged, skipped {} ticks", skipped);
                        }
                        Err(_) => {
                            break;
                        }
                    }
                }
            }
        }
    };

    sse_response(StreamBody::new(stream))
}

// ============================================================================

// LLM Metrics
// ============================================================================

/// Query 参数：可选 system_hash hex 过滤
#[derive(Deserialize, Default, Debug, Clone)]
pub struct MetricsQuery {
    pub system_hash: Option<String>,
}

/// GET /api/v1/metrics — LLM 性能指标
pub async fn get_metrics_handler(Query(q): Query<MetricsQuery>) -> Json<serde_json::Value> {
    use crate::component::llm::snapshot_all_stats;

    let mut stats = snapshot_all_stats();
    if let Some(hash_hex) = q.system_hash.as_deref()
        && let Ok(bytes) = hex::decode(hash_hex)
        && bytes.len() == 32
    {
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        stats.retain(|s| s.system_hash_distribution.contains_key(&arr));
    }

    let mut total_prompt: u64 = 0;
    let mut total_cache_hit: u64 = 0;
    // 场景归因聚合（跨模型求和）：回答"token 花在哪个环节"
    let mut by_scenario: std::collections::BTreeMap<String, (u64, u64, u64, u64)> =
        std::collections::BTreeMap::new();

    let models: Vec<serde_json::Value> = stats
        .iter()
        .map(|s| {
            let success_rate = if s.calls > 0 {
                (s.calls - s.failures) as f64 / s.calls as f64
            } else {
                1.0
            };
            let cache_hit_rate = if s.prompt_tokens > 0 {
                s.cache_hit_tokens as f64 / s.prompt_tokens as f64
            } else {
                0.0
            };
            total_prompt += s.prompt_tokens;
            total_cache_hit += s.cache_hit_tokens;
            for (sc, b) in &s.by_scenario {
                let e = by_scenario.entry(sc.clone()).or_default();
                e.0 += b.prompt_tokens;
                e.1 += b.completion_tokens;
                e.2 += b.cache_hit_tokens;
                e.3 += b.calls;
            }
            serde_json::json!({
                "provider": s.provider,
                "model": s.model,
                "calls": s.calls,
                "failures": s.failures,
                "success_rate": format!("{:.0}%", success_rate * 100.0),
                "prompt_tokens": s.prompt_tokens,
                "completion_tokens": s.completion_tokens,
                "total_tokens": s.prompt_tokens + s.completion_tokens,
                "cache_hit_tokens": s.cache_hit_tokens,
                "cache_hit_rate": format!("{:.1}%", cache_hit_rate * 100.0),
                "by_scenario": s.by_scenario,
            })
        })
        .collect();

    let scenarios: Vec<serde_json::Value> = by_scenario
        .into_iter()
        .map(|(sc, (pt, ct, ch, calls))| {
            serde_json::json!({
                "scenario": sc,
                "prompt_tokens": pt,
                "completion_tokens": ct,
                "total_tokens": pt + ct,
                "cache_hit_tokens": ch,
                "calls": calls,
            })
        })
        .collect();

    let tool_rounds: Vec<serde_json::Value> =
        crate::component::llm::scenario::snapshot_tool_rounds()
            .into_iter()
            .map(|(round, calls)| serde_json::json!({ "round": round, "calls": calls }))
            .collect();

    let overall_cache_hit_rate = if total_prompt > 0 {
        total_cache_hit as f64 / total_prompt as f64
    } else {
        0.0
    };

    Json(serde_json::json!({
        "llm": models,
        "scenarios": scenarios,
        "tool_rounds": tool_rounds,
        "forced_text_exits": crate::component::llm::scenario::snapshot_forced_text_exits(),
        "total_cache_hit_tokens": total_cache_hit,
        "total_prompt_tokens": total_prompt,
        "cache_hit_rate": format!("{:.1}%", overall_cache_hit_rate * 100.0),
        // 决策模型路径计量（非 LLM API 调用，独立标记来源）
        "decision_model": crate::component::decision_model::metrics::snapshot(),
    }))
}
