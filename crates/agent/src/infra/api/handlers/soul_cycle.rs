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

/// 托梦请求
#[derive(Debug, Deserialize)]
pub struct DreamRequest {
    /// 念头内容（注入到上下文）
    pub thought: String,
    /// 持续回合数
    #[serde(default = "default_dream_duration")]
    pub duration: u32,
}

fn default_dream_duration() -> u32 {
    5
}

/// 托梦响应
#[derive(Debug, Serialize)]
pub struct DreamResponse {
    /// 是否成功
    pub success: bool,
    /// 消息
    pub message: String,
    /// 剩余回合数
    pub remaining_ticks: u32,
    /// 今天是否还能使用
    pub can_use_today: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamRecord {
    pub injected_at: String,
    pub thought: String,
    pub duration: u32,
}

/// Compute dream data directory for a specific character.
/// Returns `character_dir / agent_id / data`.
async fn dream_data_dir(state: &HttpApiState, agent_id: uuid::Uuid) -> std::path::PathBuf {
    state
        .character_dir
        .read()
        .await
        .join(agent_id.to_string())
        .join("data")
}

/// 托梦状态（存储在 HttpApiState 中）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DreamState {
    /// 当前托梦内容
    pub thought: Option<String>,
    /// 剩余回合数
    pub remaining_ticks: u32,
    pub records: Vec<DreamRecord>,
    /// 上次使用的游戏日期（用于每日限制）
    pub last_used_game_date: Option<GameDate>,
    #[serde(skip)]
    pub loaded: bool,
    #[serde(skip)]
    pub current_agent_id: Option<uuid::Uuid>,
}

impl DreamState {
    pub fn load_from_file(data_dir: &std::path::Path, agent_id: &uuid::Uuid) -> Option<Self> {
        if agent_id.is_nil() {
            return None;
        }
        let file_path = data_dir.join(format!("dream_state_{}.json", agent_id));
        if file_path.exists() {
            match std::fs::read_to_string(&file_path) {
                Ok(content) => match serde_json::from_str::<Self>(&content) {
                    Ok(mut state) => {
                        state.loaded = true;
                        state.current_agent_id = Some(*agent_id);
                        return Some(state);
                    }
                    Err(e) => {
                        tracing::error!("反序列化托梦记录失败 {:?}: {}", file_path, e);
                    }
                },
                Err(e) => {
                    tracing::error!("读取托梦记录文件失败 {:?}: {}", file_path, e);
                }
            }
        }
        None
    }

    pub fn save_to_file(&self, data_dir: &std::path::Path, agent_id: &uuid::Uuid) {
        if agent_id.is_nil() {
            return;
        }
        if let Err(e) = std::fs::create_dir_all(data_dir) {
            tracing::error!("创建托梦数据目录失败 {:?}: {}", data_dir, e);
            return;
        }
        let file_path = data_dir.join(format!("dream_state_{}.json", agent_id));
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&file_path, json) {
                    tracing::error!("写入托梦记录文件失败 {:?}: {}", file_path, e);
                }
            }
            Err(e) => {
                tracing::error!("序列化托梦记录失败: {}", e);
            }
        }
    }

    pub fn ensure_loaded(&mut self, data_dir: &std::path::Path, agent_id: &uuid::Uuid) {
        if agent_id.is_nil() {
            return;
        }
        if self.loaded && self.current_agent_id == Some(*agent_id) {
            return;
        }
        if let Some(loaded) = Self::load_from_file(data_dir, agent_id) {
            *self = loaded;
        } else {
            self.thought = None;
            self.remaining_ticks = 0;
            self.records.clear();
            self.last_used_game_date = None;
            self.loaded = true;
            self.current_agent_id = Some(*agent_id);
        }
    }
}

/// 游戏日期（用于每日限制）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GameDate {
    pub year: i32,
    pub month: i32,
    pub day: i32,
}

impl GameDate {
    pub fn from_world_time(world_time: &cyber_jianghu_protocol::WorldTime) -> Self {
        Self {
            year: world_time.year,
            month: world_time.month,
            day: world_time.day,
        }
    }
}

/// 托梦（持续 n 回合的念头注入）
///
/// POST /api/v1/character/dream
///
/// 将念头注入到 Agent 的上下文中，持续指定回合数
pub(crate) async fn dream_character_handler(
    State(state): State<HttpApiState>,
    Json(req): Json<DreamRequest>,
) -> impl IntoResponse {
    use tracing::info;

    // 检查是否有托梦存储
    let dream_store = match &state.dream_store {
        Some(store) => store,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(DreamResponse {
                    success: false,
                    message: "托梦功能未初始化".to_string(),
                    remaining_ticks: 0,
                    can_use_today: false,
                }),
            )
                .into_response();
        }
    };

    // 获取当前 WorldState
    let current = state.current_state.read().await;
    let ws = match current.as_ref() {
        Some(ws) => ws,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(DreamResponse {
                    success: false,
                    message: "游戏状态尚未加载".to_string(),
                    remaining_ticks: 0,
                    can_use_today: false,
                }),
            )
                .into_response();
        }
    };

    let current_date = GameDate::from_world_time(&ws.world_time);

    // 检查每日限制
    {
        let mut dream = dream_store.write().await;
        let agent_id = *state.agent_id.read().await;
        let dd = dream_data_dir(&state, agent_id).await;
        dream.ensure_loaded(&dd, &agent_id);

        if let Some(ref last_date) = dream.last_used_game_date
            && last_date == &current_date
        {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(DreamResponse {
                    success: false,
                    message: "今日已使用过托梦，请明天再试".to_string(),
                    remaining_ticks: dream.remaining_ticks,
                    can_use_today: false,
                }),
            )
                .into_response();
        }
    }

    info!(
        "托梦注入: thought={}, duration={}, game_date={}-{}-{}",
        req.thought, req.duration, current_date.year, current_date.month, current_date.day
    );

    // 更新托梦状态
    let mut dream = dream_store.write().await;
    let agent_id = *state.agent_id.read().await;
    let dd = dream_data_dir(&state, agent_id).await;
    dream.ensure_loaded(&dd, &agent_id);

    dream.thought = Some(req.thought.clone());
    dream.remaining_ticks = req.duration;
    dream.last_used_game_date = Some(current_date);
    dream.records.insert(
        0,
        DreamRecord {
            injected_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            thought: req.thought.clone(),
            duration: req.duration,
        },
    );
    // dream records 容量不设上限，由 dream duration 自然限制
    dream.save_to_file(&dream_data_dir(&state, agent_id).await, &agent_id);

    Json(DreamResponse {
        success: true,
        message: format!("托梦成功，将持续 {} 回合", req.duration),
        remaining_ticks: req.duration,
        can_use_today: false, // 刚用过，今天不能再用了
    })
    .into_response()
}

/// 获取当前托梦状态
///
/// GET /api/v1/character/dream
pub(crate) async fn get_dream_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let dream_store = match &state.dream_store {
        Some(store) => store,
        None => {
            return Json(DreamStatusResponse {
                thought: None,
                remaining_ticks: 0,
                can_use_today: true,
            })
            .into_response();
        }
    };

    let mut dream = dream_store.write().await;
    let agent_id = *state.agent_id.read().await;
    let dd = dream_data_dir(&state, agent_id).await;
    dream.ensure_loaded(&dd, &agent_id);

    // 获取当前游戏日期，判断今天是否还能使用
    let can_use_today = {
        let current = state.current_state.read().await;
        match current.as_ref() {
            Some(ws) => {
                let current_date = GameDate::from_world_time(&ws.world_time);
                dream.last_used_game_date.as_ref() != Some(&current_date)
            }
            None => true, // 没有状态时默认可用
        }
    };

    Json(DreamStatusResponse {
        thought: dream.thought.clone(),
        remaining_ticks: dream.remaining_ticks,
        can_use_today,
    })
    .into_response()
}

/// 托梦状态响应
#[derive(Debug, Serialize)]
pub struct DreamStatusResponse {
    /// 当前托梦内容
    pub thought: Option<String>,
    /// 剩余回合数
    pub remaining_ticks: u32,
    /// 今天是否还能使用
    pub can_use_today: bool,
}

#[derive(Debug, Serialize)]
pub struct DreamRecordsResponse {
    pub page: u32,
    pub limit: u32,
    pub total: u32,
    pub has_more: bool,
    pub records: Vec<DreamRecord>,
}

pub(crate) async fn get_dream_records_handler(
    State(state): State<HttpApiState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let page: u32 = params.get("page").and_then(|s| s.parse().ok()).unwrap_or(1);
    let limit: u32 = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);

    let Some(dream_store) = &state.dream_store else {
        return Json(DreamRecordsResponse {
            page,
            limit,
            total: 0,
            has_more: false,
            records: vec![],
        })
        .into_response();
    };

    let mut dream = dream_store.write().await;
    let agent_id = *state.agent_id.read().await;
    let dd = dream_data_dir(&state, agent_id).await;
    dream.ensure_loaded(&dd, &agent_id);

    let total = dream.records.len() as u32;
    let start = ((page - 1) * limit) as usize;
    let end = std::cmp::min(start + limit as usize, dream.records.len());
    let records = if start < dream.records.len() {
        dream.records[start..end].to_vec()
    } else {
        vec![]
    };

    Json(DreamRecordsResponse {
        page,
        limit,
        total,
        has_more: end < dream.records.len(),
        records,
    })
    .into_response()
}

// ============================================================================
