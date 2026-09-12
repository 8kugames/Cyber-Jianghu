// SSE state/stream Handler
// ============================================================================
//
// GET /api/v1/state/stream — WorldState + IntentSnapshot 复合 SSE 流（桌面 client 消费）。
//
// 契约以 client 仓 tools/mock_sse_server.py + godot ViewBuilder 为可执行权威，
// JSON Schema 片段见主仓 docs/contracts/state_stream.schema.json：
//   event: connected   data: {"ok":true}
//   event: state       data: {"world_state":{...view...}, "intent_snapshot":null}
//   event: heartbeat   data: {"ts":<unix秒>}（空闲 30s 一次）
//
// world_state 为 view 形态（非原始协议 WorldState）：
//   tick_id / protagonist_id / protagonist{id,name,location_type,action_type,vitality,mood}
//   nearby[{id,name,distance,action_type}] / headline_events[{tick_id,event_type,actor_id,target_id,summary}]
//   intent_snapshot{current_thought,expires_at_tick}|null —— 托梦视图
// 顶层 intent_snapshot 为保留字段，当前恒为 null（client 现阶段忽略）。

use std::convert::Infallible;
use std::time::Duration;

use axum::{body::Body, extract::State, http::StatusCode, response::Response};
use bytes::Bytes;
use http_body::Frame;
use http_body_util::StreamBody;

use cyber_jianghu_protocol::WorldState;

use super::HttpApiState;

/// 空闲心跳间隔（client 契约：30s 一次）
const HEARTBEAT_INTERVAL_SECS: u64 = 30;
/// state 事件中 headline_events 的条数上限（控制单帧体积）
const HEADLINE_EVENTS_LIMIT: usize = 10;

/// GET /api/v1/state/stream — SSE 复合状态流
///
/// 订阅 tick_update 广播，每个 tick 推送一帧 state；空闲心跳保活。
/// 鉴权由 auth 中间件统一处理（Bearer header 或本端点专属 query token）。
pub(crate) async fn state_stream_handler(State(state): State<HttpApiState>) -> Response {
    let mut tick_rx = state.tick_update_tx.subscribe();

    let stream = async_stream::stream! {
        let data = Bytes::from_static(b"event: connected\ndata: {\"ok\":true}\n\n");
        yield Ok::<_, Infallible>(Frame::data(data));

        // 连接建立后立即推送当前状态（若有），避免 client 等待下一个 tick
        if let Some(data) = build_state_event_data(&state).await {
            yield Ok::<_, Infallible>(Frame::data(data));
        }

        loop {
            tokio::select! {
                tick_result = tick_rx.recv() => {
                    match tick_result {
                        Ok(_) => {
                            if let Some(data) = build_state_event_data(&state).await {
                                yield Ok::<_, Infallible>(Frame::data(data));
                            }
                        }
                        // 消费端落后：丢弃的是旧帧，通道仍活，继续跟流（不可断连）
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::warn!("[state_stream] broadcast lagged, skipped {} ticks", skipped);
                        }
                        // 通道关闭（进程停止）
                        Err(_) => break,
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(HEARTBEAT_INTERVAL_SECS)) => {
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let payload = serde_json::json!({"ts": ts}).to_string();
                    let data = Bytes::from(format!("event: heartbeat\ndata: {}\n\n", payload));
                    yield Ok::<_, Infallible>(Frame::data(data));
                }
            }
        }
    };

    let body = StreamBody::new(stream);
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream; charset=utf-8")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .header("X-Accel-Buffering", "no")
        .body(Body::new(body))
        .expect("valid HTTP response")
}

/// view 构建的输入依赖（与 HttpApiState 解耦，便于单测）
struct StreamViewDeps<'a> {
    protagonist_name: &'a str,
    mood: &'a str,
    /// 最近一次执行的动作类型（decision snapshot），None → "idle"
    last_action: Option<&'a str>,
    /// 活跃托梦 (thought, remaining_ticks)，None → intent_snapshot 为 null
    dream: Option<(&'a str, u32)>,
}

/// 组装 state 事件的完整 data 帧（world_state 尚未加载时返回 None）
async fn build_state_event_data(state: &HttpApiState) -> Option<Bytes> {
    let ws = state.current_state.read().await.clone()?;

    let (protagonist_name, mood) = {
        let guard = state.dynamic_persona.read().expect("rwlock poisoned");
        match guard.as_ref() {
            Some(p) => (
                p.read(|d| d.name.clone()),
                p.read(|d| d.current_state.current_emotion.clone()),
            ),
            None => (String::new(), String::new()),
        }
    };
    let last_action = state
        .decision_context_snapshot
        .read()
        .await
        .as_ref()
        .and_then(|s| {
            s.last_execution_result
                .as_ref()
                .map(|e| e.action_type.clone())
        });
    let dream = read_active_dream(state).await;

    let deps = StreamViewDeps {
        protagonist_name: &protagonist_name,
        mood: &mood,
        last_action: last_action.as_deref(),
        dream: dream.as_ref().map(|(t, r)| (t.as_str(), *r)),
    };

    let payload = serde_json::json!({
        "world_state": build_world_state_view(&ws, &deps),
        "intent_snapshot": null, // 保留字段（client 现阶段忽略）
    });
    Some(Bytes::from(format!("event: state\ndata: {}\n\n", payload)))
}

/// 读取当前角色的活跃托梦 (thought, remaining_ticks)；无托梦存储或无活跃托梦返回 None
async fn read_active_dream(state: &HttpApiState) -> Option<(String, u32)> {
    let store = state.dream_store.as_ref()?;
    let agent_id = *state.agent_id.read().await;
    // 先算目录再取写锁，避免 dream 写锁跨 await 点
    let dd = super::dream::dream_data_dir(state, agent_id).await;
    let mut dream = store.write().await;
    dream.ensure_loaded(&dd, &agent_id);
    match dream.thought.clone() {
        Some(t) if !t.is_empty() && dream.remaining_ticks > 0 => Some((t, dream.remaining_ticks)),
        _ => None,
    }
}

/// 将原始 WorldState 映射为 client ViewBuilder 契约的 view 形态（纯函数）
fn build_world_state_view(ws: &WorldState, deps: &StreamViewDeps) -> serde_json::Value {
    let protagonist_id = ws.agent_id.unwrap_or_default().to_string();

    let protagonist = serde_json::json!({
        "id": protagonist_id,
        "name": deps.protagonist_name,
        "location_type": ws.location.node_type,
        "action_type": deps.last_action.unwrap_or("idle"),
        "vitality": ws.self_state.hp(),
        "mood": deps.mood,
    });

    let nearby: Vec<serde_json::Value> = ws
        .entities
        .iter()
        .map(|e| {
            serde_json::json!({
                "id": e.id.to_string(),
                "name": e.name,
                "distance": e.distance,
                "action_type": e.recent_actions.last().map(|a| a.action_type.clone()).unwrap_or_default(),
            })
        })
        .collect();

    let headline_events: Vec<serde_json::Value> = ws
        .events_log
        .iter()
        .take(HEADLINE_EVENTS_LIMIT)
        .map(|ev| {
            serde_json::json!({
                "tick_id": ev.tick_id,
                "event_type": ev.event_type.as_str(),
                "actor_id": ev.metadata.get("actor_id").and_then(|v| v.as_str()),
                "target_id": ev.metadata.get("target_id").and_then(|v| v.as_str()),
                "summary": ev.description,
            })
        })
        .collect();

    let intent_snapshot = deps.dream.map(|(thought, remaining)| {
        serde_json::json!({
            "current_thought": thought,
            "expires_at_tick": ws.tick_id + remaining as i64,
        })
    });

    serde_json::json!({
        "tick_id": ws.tick_id,
        "protagonist_id": protagonist_id,
        "protagonist": protagonist,
        "nearby": nearby,
        "headline_events": headline_events,
        "intent_snapshot": intent_snapshot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_jianghu_protocol::{
        AgentSelfState, Entity, Location, RecentAction, WorldEvent, WorldEventType, WorldTime,
    };
    use std::collections::HashMap;
    use uuid::Uuid;

    fn make_world_state() -> WorldState {
        WorldState {
            event_type: "world_state".to_string(),
            tick_id: 42,
            agent_id: Some(Uuid::new_v4()),
            world_time: WorldTime {
                year: 1,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
                weather: String::new(),
            },
            location: Location {
                node_id: "loc_1".to_string(),
                name: "破庙".to_string(),
                node_type: "shrine".to_string(),
                adjacent_nodes: vec![],
                gatherable_items: vec![],
                parent_chain: Vec::new(),
            },
            self_state: AgentSelfState {
                attributes: HashMap::from([("hp".to_string(), 85)]),
                derived_attributes: HashMap::new(),
                attribute_descriptions: HashMap::new(),
                survival_drives: vec![],
                status_effects: vec![],
                inventory: vec![],
                skills: vec![],
                recipe_details: vec![],
                age_years: None,
                max_age: None,
            },
            entities: vec![Entity {
                id: Uuid::new_v4(),
                name: "赵灵儿".to_string(),
                distance: 0,
                state: "alive".to_string(),
                hostile: false,
                recent_actions: vec![RecentAction {
                    tick_id: 41,
                    action_type: "说话".to_string(),
                    content: None,
                    result: "打了招呼".to_string(),
                }],
            }],
            nearby_items: vec![],
            events_log: vec![WorldEvent {
                event_type: WorldEventType::PublicMessage,
                tick_id: 41,
                description: "李逍遥对赵灵儿说：酒不错".to_string(),
                metadata: serde_json::json!({"actor_id": "p1", "target_id": "n1"}),
            }],
            private_dialogue_log: vec![],
            last_execution_summary: None,
        }
    }

    #[test]
    fn view_shape_matches_client_contract() {
        let ws = make_world_state();
        let deps = StreamViewDeps {
            protagonist_name: "李逍遥",
            mood: "平静",
            last_action: Some("饮酒"),
            dream: None,
        };
        let v = build_world_state_view(&ws, &deps);

        assert_eq!(v["tick_id"], 42);
        assert_eq!(
            v["protagonist_id"],
            ws.agent_id.unwrap().to_string().as_str()
        );
        assert_eq!(v["protagonist"]["name"], "李逍遥");
        assert_eq!(v["protagonist"]["location_type"], "shrine");
        assert_eq!(v["protagonist"]["action_type"], "饮酒");
        assert_eq!(v["protagonist"]["vitality"], 85);
        assert_eq!(v["protagonist"]["mood"], "平静");

        assert_eq!(v["nearby"][0]["name"], "赵灵儿");
        assert_eq!(v["nearby"][0]["action_type"], "说话");
        assert_eq!(v["nearby"][0]["distance"], 0);

        assert_eq!(v["headline_events"][0]["event_type"], "public_message");
        assert_eq!(v["headline_events"][0]["actor_id"], "p1");
        assert_eq!(v["headline_events"][0]["target_id"], "n1");
        assert_eq!(
            v["headline_events"][0]["summary"],
            "李逍遥对赵灵儿说：酒不错"
        );

        // 无托梦 → intent_snapshot 为 null
        assert!(v["intent_snapshot"].is_null());
    }

    #[test]
    fn view_defaults_when_no_deps() {
        let ws = make_world_state();
        let deps = StreamViewDeps {
            protagonist_name: "",
            mood: "",
            last_action: None,
            dream: None,
        };
        let v = build_world_state_view(&ws, &deps);
        assert_eq!(v["protagonist"]["action_type"], "idle");
        // nearby 的 action_type 来自实体自身 recent_actions，与 deps 无关：
        // 夹具实体带一条 recent_action，故取最后一条
        assert_eq!(v["nearby"][0]["action_type"], "说话");
        assert_eq!(v["protagonist"]["name"], "");
    }

    #[test]
    fn view_dream_maps_to_intent_snapshot() {
        let ws = make_world_state();
        let deps = StreamViewDeps {
            protagonist_name: "李逍遥",
            mood: "平静",
            last_action: None,
            dream: Some(("想再去一趟桃花岛", 5)),
        };
        let v = build_world_state_view(&ws, &deps);
        assert_eq!(v["intent_snapshot"]["current_thought"], "想再去一趟桃花岛");
        assert_eq!(v["intent_snapshot"]["expires_at_tick"], 42 + 5);
    }
}
