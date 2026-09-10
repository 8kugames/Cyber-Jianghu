// 三魂循环记录 API
// ============================================================================

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::{Deserialize, Serialize};
use tracing::{error, warn};
use uuid::Uuid;

use crate::config::{CharacterConfig, CharacterStatus};

use super::HttpApiState;
use super::character_helpers::get_device_id;
use super::character_info::enrich_world_time_json;

/// Layer 结果条目
#[derive(Debug, Serialize)]
struct LayerResultEntry {
    layer: String,
    passed: bool,
    detail: Option<String>,
}

/// 人魂记录
#[derive(Debug, Serialize)]
struct RenhunEntry {
    narrative: Option<String>,
    thought_log: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    earth_tool_calls: Option<Vec<cyber_jianghu_protocol::EarthToolCall>>,
}

/// 天魂审查记录
#[derive(Debug, Serialize)]
struct TianhunEntry {
    result: Option<String>,
    layers: Vec<LayerResultEntry>,
    reason: Option<String>,
}

/// 最终 Intent 记录
#[derive(Debug, Serialize)]
struct FinalIntentEntry {
    intent_id: Option<String>,
    action_type: Option<String>,
    action_data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pipeline_actions: Option<Vec<cyber_jianghu_protocol::PipelineAction>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dream_marker: Option<serde_json::Value>,
}

/// 单条三魂尝试记录
#[derive(Debug, Serialize)]
struct SoulCycleAttemptEntry {
    tick_id: i64,
    world_time: Option<serde_json::Value>,
    created_at: String,
    attempt: i32,
    renhun: RenhunEntry,
    tianhun: TianhunEntry,
    final_intent: Option<FinalIntentEntry>,
    /// 该次尝试使用的 LLM 模型 ID（用于经历日志展示）
    #[serde(skip_serializing_if = "Option::is_none")]
    model_id: Option<String>,
    /// Server 执行结果回填（数据驱动，key=pipe_seq）
    #[serde(skip_serializing_if = "Option::is_none")]
    execution_results: Option<serde_json::Value>,
}

/// 即时意图记录
#[derive(Debug, Serialize)]
struct ImmediateIntentEntry {
    intent_id: String,
    route_type: String,
    action_type: String,
    action_data: Option<serde_json::Value>,
    speech_content: Option<String>,
    send_status: String,
    send_error: Option<String>,
}

/// 三魂循环完整记录响应
#[derive(Debug, Serialize)]
struct SoulCyclesResponse {
    tick_id: i64,
    attempts: Vec<SoulCycleAttemptEntry>,
    immediate_intents: Vec<ImmediateIntentEntry>,
}

/// 三魂循环分页响应（按 tick 分组）
#[derive(Debug, Serialize)]
struct SoulCyclesPageResponse {
    page: u32,
    limit: u32,
    total: u32,
    has_more: bool,
    records: std::collections::HashMap<String, Vec<SoulCycleAttemptEntry>>,
    immediate_intents: std::collections::HashMap<String, Vec<ImmediateIntentEntry>>,
}

/// SoulCycleRecord → SoulCycleAttemptEntry 转换（消除重复代码）
fn record_to_attempt_entry(
    r: super::soul_cycle_recorder::SoulCycleRecord,
) -> SoulCycleAttemptEntry {
    let action_data: Option<serde_json::Value> = r
        .final_action_data
        .as_ref()
        .and_then(|s| serde_json::from_str(s).ok());
    let layers = [
        (r.tianhun_layer1_result.as_deref(), "layer1"),
        (r.tianhun_layer2_result.as_deref(), "layer2"),
        (r.tianhun_layer3_result.as_deref(), "layer3"),
    ]
    .iter()
    .map(|(detail, layer)| {
        let passed = detail.map(|d| d == "通过" || d.is_empty()).unwrap_or(true);
        LayerResultEntry {
            layer: layer.to_string(),
            passed,
            detail: if passed {
                None
            } else {
                Some(detail.unwrap_or("驳回").to_string())
            },
        }
    })
    .collect();
    let world_time: Option<serde_json::Value> = r.world_time.as_ref().and_then(|s| {
        let parsed: Option<cyber_jianghu_protocol::WorldTime> = serde_json::from_str(s).ok();
        match parsed {
            Some(wt) => enrich_world_time_json(&wt),
            None => Some(serde_json::Value::String(s.clone())),
        }
    });

    SoulCycleAttemptEntry {
        tick_id: r.tick_id,
        world_time,
        created_at: r.created_at.to_rfc3339(),
        attempt: r.attempt,
        renhun: RenhunEntry {
            narrative: r.renhun_narrative,
            thought_log: r.renhun_thought_log,
            earth_tool_calls: r
                .earth_tool_calls
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok()),
        },
        tianhun: TianhunEntry {
            result: r.tianhun_result,
            layers,
            reason: r.tianhun_reason,
        },
        final_intent: r.final_intent_id.map(|id| {
            let pipeline_actions: Option<Vec<cyber_jianghu_protocol::PipelineAction>> = r
                .final_pipeline_json
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok());
            FinalIntentEntry {
                intent_id: Some(id),
                action_type: r.final_action_type,
                action_data,
                pipeline_actions,
                dream_marker: None,
            }
        }),
        model_id: r.model_id,
        execution_results: r
            .server_execution_results
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok()),
    }
}

/// ImmediateIntentRecord → ImmediateIntentEntry 转换
fn immediate_record_to_entry(
    r: super::soul_cycle_recorder::ImmediateIntentRecord,
) -> ImmediateIntentEntry {
    let action_data: Option<serde_json::Value> = r
        .action_data
        .as_ref()
        .and_then(|s| serde_json::from_str(s).ok());
    ImmediateIntentEntry {
        intent_id: r.intent_id,
        route_type: r.route_type,
        action_type: r.action_type,
        action_data,
        speech_content: r.speech_content,
        send_status: r.send_status,
        send_error: r.send_error,
    }
}

/// 获取指定角色的三魂完整记录
///
/// GET /api/v1/character/soul-cycles?tick_id=123
/// GET /api/v1/character/soul-cycles?page=1&limit=20
/// GET /api/v1/character/soul-cycles?agent_id=xxx&page=1&limit=20  # 指定角色
pub(crate) async fn get_soul_cycles_handler(
    State(state): State<HttpApiState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
    let tick_id: Option<i64> = params.get("tick_id").and_then(|s| s.parse().ok());
    let page: u32 = params.get("page").and_then(|s| s.parse().ok()).unwrap_or(1);
    let limit: u32 = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(20)
        .min(50);

    // 确定查询目标角色：优先使用 agent_id 参数，否则用当前角色
    let target_agent_id = if let Some(id_str) = params.get("agent_id") {
        match uuid::Uuid::parse_str(id_str) {
            Ok(id) => id,
            Err(_) => {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "Invalid agent_id format"})),
                )
                    .into_response();
            }
        }
    } else {
        *state.agent_id.read().await
    };

    let Some(recorder) = state.soul_recorder_for(target_agent_id).await else {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Soul cycle record not found for this agent"})),
        )
            .into_response();
    };

    if let Some(tid) = tick_id {
        // 按 tick_id 查询
        let records = match recorder.get_by_tick(tid).await {
            Ok(r) => r,
            Err(e) => {
                return (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("get_by_tick({tid}) 失败: {e:?}")
                    })),
                )
                    .into_response();
            }
        };
        let immediate = match recorder.get_immediate_by_tick(tid).await {
            Ok(r) => r,
            Err(e) => {
                return (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("get_immediate_by_tick({tid}) 失败: {e:?}")
                    })),
                )
                    .into_response();
            }
        };

        let attempts: Vec<SoulCycleAttemptEntry> =
            records.into_iter().map(record_to_attempt_entry).collect();

        let immediate_intents: Vec<ImmediateIntentEntry> = immediate
            .into_iter()
            .map(immediate_record_to_entry)
            .collect();

        Json(SoulCyclesResponse {
            tick_id: tid,
            attempts,
            immediate_intents,
        })
        .into_response()
    } else {
        // 分页查询：按 tick_id 分组
        let (tick_ids, total) = match recorder.get_tick_ids_page(page, limit).await {
            Ok(r) => r,
            Err(e) => {
                return (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("get_tick_ids_page 失败: {e:?}")
                    })),
                )
                    .into_response();
            }
        };

        // 批量获取所有 tick 的记录和即时意图
        let all_records = match recorder.get_by_ticks(&tick_ids).await {
            Ok(r) => r,
            Err(e) => {
                return (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("get_by_ticks 失败: {e:?}")
                    })),
                )
                    .into_response();
            }
        };
        let all_immediate = match recorder.get_immediate_by_ticks(&tick_ids).await {
            Ok(r) => r,
            Err(e) => {
                return (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("get_immediate_by_ticks 失败: {e:?}")
                    })),
                )
                    .into_response();
            }
        };

        // 按 tick_id 分组记录
        let mut records_map: std::collections::HashMap<String, Vec<SoulCycleAttemptEntry>> =
            std::collections::HashMap::new();
        for r in all_records {
            let tick_key = r.tick_id.to_string();
            let entry = record_to_attempt_entry(r);
            records_map.entry(tick_key).or_default().push(entry);
        }

        // 按 tick_id 分组即时意图
        let mut immediate_map: std::collections::HashMap<String, Vec<ImmediateIntentEntry>> =
            std::collections::HashMap::new();
        for imm in all_immediate {
            let tick_key = imm.tick_id.to_string();
            let entry = immediate_record_to_entry(imm);
            immediate_map.entry(tick_key).or_default().push(entry);
        }

        let has_more = (page * limit) < total;
        Json(SoulCyclesPageResponse {
            page,
            limit,
            total,
            has_more,
            records: records_map,
            immediate_intents: immediate_map,
        })
        .into_response()
    }
}

/// 重生请求
#[derive(Debug, Deserialize)]
pub struct RebirthRequest {
    /// 确认重生
    pub confirm: bool,
}

/// 重生响应
#[derive(Debug, Serialize)]
pub struct RebirthResponse {
    /// 是否成功
    pub success: bool,
    /// 消息
    pub message: String,
}

/// 重生：从终态（dead/retired/active）创建新角色
///
/// POST /api/v1/character/rebirth
///
/// 流程：
/// 1. 调用 server /api/v1/agent/retire（幂等：active→retired，dead/retired→no-op）
/// 2. 清理本地状态（文件系统 + 内存）
/// 3. 触发 WebSocket 重连 → 进入角色创建流程
pub(crate) async fn rebirth_character_handler(
    State(state): State<HttpApiState>,
    Json(req): Json<RebirthRequest>,
) -> impl IntoResponse {
    use tracing::info;

    if !req.confirm {
        return (
            StatusCode::BAD_REQUEST,
            Json(RebirthResponse {
                success: false,
                message: "请确认重生操作 (confirm: true)".to_string(),
            }),
        )
            .into_response();
    }

    // 1. 获取设备身份
    let (device_id, auth_token) = match get_device_id(&state).await {
        Ok(id) => id,
        Err(e) => {
            return (
                StatusCode::PRECONDITION_FAILED,
                Json(RebirthResponse {
                    success: false,
                    message: format!("设备身份未初始化: {}", e),
                }),
            )
                .into_response();
        }
    };

    let agent_id = *state.agent_id.read().await;
    info!(
        "[rebirth] 角色重生: agent_id={}, device_id={}",
        agent_id, device_id
    );

    // 数据驱动 dispatch：按当前 agent 状态选 server 端点
    // 读取 character.yaml 中的 CharacterStatus（数据驱动）：
    // - Dead: 调 /api/v1/agent/auto-rebirth（创建新 agent，旧 agent 保持 status='dead'，用户裁决默认行为）
    // - Alive: 调 /api/v1/agent/retire（玩家主动归隐）
    // - Retired: 幂等 no-op（已是归隐状态）
    // - 未找到 character.yaml 或 agent_id 为 nil：默认 no-op（避免对未知角色误调 retire 导致错误归隐）
    let character_status = if agent_id != Uuid::nil() {
        let characters_dir = state.character_dir.read().await.clone();
        let char_yaml = characters_dir
            .join(agent_id.to_string())
            .join("character.yaml");
        match crate::config::CharacterConfig::from_file(&char_yaml) {
            Ok(c) => c.status,
            Err(_) => {
                // character.yaml 不存在或损坏：保守 no-op
                warn!(
                    "[rebirth] character.yaml 不存在或损坏: agent={}, 跳过 server 调用",
                    agent_id
                );
                return Json(RebirthResponse {
                    success: true,
                    message: "无法读取角色状态，请重试或手动操作".to_string(),
                })
                .into_response();
            }
        }
    } else {
        crate::config::CharacterStatus::Retired
    };

    let client = reqwest::Client::new();
    let server_http_url = state.server_http_url.read().await.clone();

    let (server_url, request_body, log_tag) = match character_status {
        crate::config::CharacterStatus::Dead => {
            // dead → auto-rebirth（创建全新 agent，old agent 保持 status='dead'）
            let url = format!("{}/api/v1/agent/auto-rebirth", server_http_url);
            let body = serde_json::json!({
                "device_id": device_id,
                "auth_token": auth_token,
                "old_agent_id": agent_id,
            });
            (url, body, "auto-rebirth (dead→保持dead, 创建新agent)")
        }
        crate::config::CharacterStatus::Alive => {
            // alive → retire（玩家主动归隐）
            let url = format!("{}/api/v1/agent/retire", server_http_url);
            let body = serde_json::json!({
                "device_id": device_id,
                "auth_token": auth_token,
            });
            (url, body, "retire (alive→retired 主动归隐)")
        }
        crate::config::CharacterStatus::Retired => {
            // 已是 retired：本地清理 + 触发重连，跳过 server 调用
            info!("[rebirth] 角色已是归隐状态，跳过 server 调用");
            return Json(RebirthResponse {
                success: true,
                message: "角色已是归隐状态，请创建新角色".to_string(),
            })
            .into_response();
        }
    };

    let response = match client.post(&server_url).json(&request_body).send().await {
        Ok(resp) => resp,
        Err(e) => {
            error!("[rebirth] 连接服务器失败: {}", e);
            return (
                StatusCode::BAD_GATEWAY,
                Json(RebirthResponse {
                    success: false,
                    message: format!("连接服务器失败: {}", e),
                }),
            )
                .into_response();
        }
    };

    let status = response.status();
    let body = response.text().await.unwrap_or_default();

    if status == reqwest::StatusCode::UNAUTHORIZED {
        error!("[rebirth] 服务器认证失败: {}", body);
        return (
            StatusCode::BAD_GATEWAY,
            Json(RebirthResponse {
                success: false,
                message: format!("服务器认证失败: {}", body),
            }),
        )
            .into_response();
    }

    if status.is_success() {
        info!(
            "[rebirth] Server 响应: 路径={}, status={}, body_len={}",
            log_tag,
            status,
            body.len()
        );
    } else {
        // 非 401 错误：仍继续本地清理（server 可能暂时不可达，但本地状态需清理）
        warn!(
            "[rebirth] Server 归隐非预期状态: status={}, body_len={}，继续本地清理",
            status,
            body.len()
        );
    }

    // 3. 清理本地文件系统：扫描 characters/ 目录，将 Alive 角色标记为 Retired
    let characters_dir = state.character_dir.read().await.clone();
    if let Ok(entries) = std::fs::read_dir(&characters_dir) {
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let char_yaml = entry.path().join("character.yaml");
            if let Ok(mut config) = CharacterConfig::from_file(&char_yaml)
                && config.status == CharacterStatus::Alive
            {
                config.status = CharacterStatus::Retired;
                if let Err(e) = config.save_to_file(&char_yaml) {
                    error!("[rebirth] 保存角色配置失败: {}", e);
                } else {
                    info!("[rebirth] 角色 '{}' 已标记为 Retired", config.name);
                }
            }
        }
    }

    // 4. 清理内存状态
    {
        let mut agent_id_guard = state.agent_id.write().await;
        *agent_id_guard = Uuid::nil();
    }
    {
        let mut current = state.current_state.write().await;
        *current = None;
    }
    // is_dead 保持 true，reconnect 成功后由注册流程设为 false

    // 5. 触发 WebSocket 重连
    if let Some(ref tx) = state.reconnect_tx {
        let server_ws_url = state.server_ws_url.read().await.clone();
        let reconnect_req = crate::infra::api::ReconnectRequest {
            ws_url: server_ws_url,
            agent_id: None,
        };
        if let Err(e) = tx.send(reconnect_req) {
            error!("[rebirth] 发送重连请求失败: {}", e);
        } else {
            info!("[rebirth] 重生完成，触发 WebSocket 重连");
        }
    }

    Json(RebirthResponse {
        success: true,
        message: "重生成功，请创建新角色".to_string(),
    })
    .into_response()
}

/// POST /api/v1/characters/{agent_id}/rebirth — 重生（client 契约的 id 路由形态）
///
/// 重生作用于已加载运行时，仅当前活跃角色可重生；id 非当前角色 → 409。
/// 语义与 POST /api/v1/character/rebirth 完全一致（委托实现）。
pub(crate) async fn rebirth_character_by_id_handler(
    State(state): State<HttpApiState>,
    axum::extract::Path(agent_id): axum::extract::Path<uuid::Uuid>,
    Json(req): Json<RebirthRequest>,
) -> axum::response::Response {
    let current = *state.agent_id.read().await;
    if agent_id != current {
        return (
            StatusCode::CONFLICT,
            Json(RebirthResponse {
                success: false,
                message: format!("character {} is not the active character", agent_id),
            }),
        )
            .into_response();
    }
    rebirth_character_handler(State(state), Json(req))
        .await
        .into_response()
}
