// 托梦(Dream Injection)API
// ============================================================================
//
// 从 soul_cycle.rs 拆出:dream 相关类型与 handler 单点维护(文件行数治理,AGENTS.md <800 行规则)。

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::{Deserialize, Serialize};

use super::HttpApiState;

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
pub(crate) async fn dream_data_dir(
    state: &HttpApiState,
    agent_id: uuid::Uuid,
) -> std::path::PathBuf {
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

/// POST /api/v1/characters/{agent_id}/inject-dream — 托梦（client 契约的 id 路由形态）
///
/// 托梦作用于已加载决策循环，仅当前活跃角色可注入；id 非当前角色 → 409。
/// 语义与 POST /api/v1/character/dream 完全一致（委托实现）。
pub(crate) async fn inject_dream_by_id_handler(
    State(state): State<HttpApiState>,
    axum::extract::Path(agent_id): axum::extract::Path<uuid::Uuid>,
    Json(req): Json<DreamRequest>,
) -> axum::response::Response {
    let current = *state.agent_id.read().await;
    if agent_id != current {
        return (
            StatusCode::CONFLICT,
            Json(DreamResponse {
                success: false,
                message: format!("character {} is not the active character", agent_id),
                remaining_ticks: 0,
                can_use_today: false,
            }),
        )
            .into_response();
    }
    dream_character_handler(State(state), Json(req))
        .await
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
