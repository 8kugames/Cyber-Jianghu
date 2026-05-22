// 多角色管理 API Handlers
// ============================================================================


use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::{Deserialize, Serialize};
use tracing::{error, info};
use uuid::Uuid;

use crate::config::CharacterStatus;


use super::HttpApiState;
use super::character_helpers::{list_characters_from_fs};

/// 角色列表响应
#[derive(Debug, Serialize)]
pub struct CharacterListResponse {
    /// 所有角色列表
    pub characters: Vec<CharacterInfo>,
    /// 当前活跃角色的 agent_id
    pub current_agent_id: Option<String>,
    /// 当前服务器 HTTP URL
    pub current_server_url: String,
}

/// 角色详细信息（用于列表展示）
#[derive(Debug, Serialize)]
pub struct CharacterInfo {
    /// 角色 ID
    pub agent_id: Option<String>,
    /// 姓名
    pub name: String,
    /// 年龄
    pub age: u8,
    /// 性别
    pub gender: String,
    /// 外貌描述
    pub appearance: Option<String>,
    /// 身份
    pub identity: Option<String>,
    /// 性格特征
    pub personality: Vec<String>,
    /// 核心价值观
    pub values: Vec<String>,
    /// 状态 (alive/dead/retired)
    pub status: String,
    /// 所属服务器 URL
    pub server_url: Option<String>,
    /// 注册时间
    pub registered_at: Option<String>,
    /// 是否为当前活跃角色
    pub is_current: bool,
    /// 最近一次连接的现实时间
    pub last_connected_real_time: Option<String>,
    /// 最近一次连接的游戏时间（格式化字符串）
    pub last_connected_world_time: Option<String>,
}

/// 获取所有角色列表
///
/// GET /api/v1/characters
///
/// 返回所有角色（包括已故、归隐的），标记当前活跃角色
pub(crate) async fn list_characters_handler(
    State(state): State<HttpApiState>,
) -> impl IntoResponse {
    // 从文件系统读取所有角色
    let characters = match list_characters_from_fs(&state.character_dir.read().await) {
        Ok(chars) => chars,
        Err(e) => {
            error!("读取角色列表失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(CharacterListResponse {
                    characters: vec![],
                    current_agent_id: None,
                    current_server_url: state.server_http_url.read().await.clone(),
                }),
            )
                .into_response();
        }
    };

    let current_server_url = state.server_http_url.read().await.clone();
    let is_dead = state.is_dead.load(std::sync::atomic::Ordering::Relaxed);
    let current_agent_id = {
        let agent_id = state.agent_id.read().await;
        if agent_id.is_nil() {
            None
        } else {
            Some(agent_id.to_string())
        }
    };

    // 构建角色列表
    let character_infos: Vec<CharacterInfo> = characters
        .iter()
        .map(|c| {
            let is_current = c.agent_id.map(|id| id.to_string()) == current_agent_id;
            // is_dead=true 时当前角色状态应显示为 dead（文件系统可能仍为 Alive）
            let status_override = if is_current && is_dead {
                Some("dead".to_string())
            } else {
                None
            };
            CharacterInfo {
                agent_id: c.agent_id.map(|id| id.to_string()),
                name: c.name.clone(),
                age: c.age,
                gender: c.gender.clone(),
                appearance: c.appearance.clone(),
                identity: c.identity.clone(),
                personality: c.personality.clone(),
                values: c.values.clone(),
                status: status_override.unwrap_or_else(|| match c.status {
                    CharacterStatus::Alive => "alive".to_string(),
                    CharacterStatus::Dead => "dead".to_string(),
                    CharacterStatus::Retired => "retired".to_string(),
                }),
                server_url: c.server_url.clone(),
                registered_at: c.registered_at.map(|t| t.to_rfc3339()),
                is_current,
                last_connected_real_time: c.last_connected_real_time.map(|t| t.to_rfc3339()),
                last_connected_world_time: c
                    .last_connected_world_time
                    .as_ref()
                    .map(|wt| wt.to_chinese()),
            }
        })
        .collect();

    Json(CharacterListResponse {
        characters: character_infos,
        current_agent_id,
        current_server_url,
    })
    .into_response()
}

/// 切换角色请求
#[derive(Debug, Deserialize)]
pub struct SwitchCharacterRequest {
    /// 目标角色的 agent_id
    pub agent_id: String,
}

/// 切换角色响应
#[derive(Debug, Serialize)]
pub struct SwitchCharacterResponse {
    pub success: bool,
    pub message: String,
    /// 切换后的角色信息
    pub character: Option<CharacterInfo>,
}

/// 切换当前活跃角色
///
/// POST /api/v1/characters/switch
///
/// 切换到指定的角色（必须是已存在的角色）
pub(crate) async fn switch_character_handler(
    State(state): State<HttpApiState>,
    Json(req): Json<SwitchCharacterRequest>,
) -> impl IntoResponse {
    // 解析 agent_id
    let agent_id = match Uuid::parse_str(&req.agent_id) {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(SwitchCharacterResponse {
                    success: false,
                    message: "无效的 agent_id 格式".to_string(),
                    character: None,
                }),
            )
                .into_response();
        }
    };

    // 从文件系统查找目标角色
    let characters = match list_characters_from_fs(&state.character_dir.read().await) {
        Ok(chars) => chars,
        Err(e) => {
            error!("读取角色列表失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SwitchCharacterResponse {
                    success: false,
                    message: format!("读取角色列表失败: {}", e),
                    character: None,
                }),
            )
                .into_response();
        }
    };

    let character = match characters.iter().find(|c| c.agent_id == Some(agent_id)) {
        Some(c) => c.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(SwitchCharacterResponse {
                    success: false,
                    message: "未找到指定的角色".to_string(),
                    character: None,
                }),
            )
                .into_response();
        }
    };

    // 检查角色状态
    if character.status != CharacterStatus::Alive {
        return (
            StatusCode::BAD_REQUEST,
            Json(SwitchCharacterResponse {
                success: false,
                message: format!(
                    "无法切换到{}角色",
                    match character.status {
                        CharacterStatus::Dead => "已故",
                        CharacterStatus::Retired => "归隐",
                        CharacterStatus::Alive => "存活",
                    }
                ),
                character: None,
            }),
        )
            .into_response();
    }

    // 更新内存中的 agent_id 并重置死亡状态
    {
        let mut current_agent_id = state.agent_id.write().await;
        *current_agent_id = agent_id;
    }
    state
        .is_dead
        .store(false, std::sync::atomic::Ordering::Relaxed);

    // 重建各类强相关的数据存储
    {
        let characters_dir = state.character_dir.read().await.clone();
        let data_dir = characters_dir.join(agent_id.to_string()).join("data");

        // 预建目录：各 DB 模块的 open() 依赖此目录存在
        if let Err(e) = std::fs::create_dir_all(&data_dir) {
            tracing::error!("切换角色失败: 无法创建数据目录 {:?} - {}", data_dir, e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SwitchCharacterResponse {
                    success: false,
                    message: format!("角色数据目录创建失败: {}", e),
                    character: None,
                }),
            )
                .into_response();
        }

        // 1. Intent History (Fail Fast)
        let new_history = match super::intent_history::IntentHistoryStore::open(
            agent_id,
            &data_dir.join(format!("intent_history_{}.db", agent_id)),
        ) {
            Ok(store) => Some(std::sync::Arc::new(store)),
            Err(e) => {
                tracing::error!("切换角色失败: 无法打开 IntentHistoryStore - {}", e);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(SwitchCharacterResponse {
                        success: false,
                        message: format!("意图历史数据库加载失败: {}", e),
                        character: None,
                    }),
                )
                    .into_response();
            }
        };
        *state.intent_history.write().await = new_history;

        // 2. Relationship Store (Fail Fast)
        let new_rel = match crate::component::social::RelationshipStore::open(
            agent_id,
            &data_dir.join(format!("relationships_{}.db", agent_id)),
        ) {
            Ok(store) => Some(std::sync::Arc::new(store)),
            Err(e) => {
                tracing::error!("切换角色失败: 无法打开 RelationshipStore - {}", e);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(SwitchCharacterResponse {
                        success: false,
                        message: format!("关系数据库加载失败: {}", e),
                        character: None,
                    }),
                )
                    .into_response();
            }
        };
        *state.relationship_store.write().unwrap() = new_rel;

        // 3. Memory Manager (Fail Fast)
        if let Some(template) = &state.memory_config_template {
            let mut config = template.clone();
            config.agent_id = agent_id;
            config.db_dir = data_dir.clone();

            let new_mem = match crate::component::memory::MemoryManager::new(config) {
                Ok(manager) => Some(std::sync::Arc::new(tokio::sync::RwLock::new(manager))),
                Err(e) => {
                    tracing::error!("切换角色失败: 无法初始化 MemoryManager - {}", e);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(SwitchCharacterResponse {
                            success: false,
                            message: format!("记忆数据库加载失败: {}", e),
                            character: None,
                        }),
                    )
                        .into_response();
                }
            };
            *state.memory_manager.write().await = new_mem;
        }

        // 4. Dream Store (按需加载，从文件读取)
        if let Some(ref dream_store) = state.dream_store {
            let mut dream = dream_store.write().await;
            if let Some(new_dream) =
                super::soul_cycle::DreamState::load_from_file(&data_dir, &agent_id)
            {
                *dream = new_dream;
            } else {
                *dream = super::soul_cycle::DreamState::default();
            }
        }
    }

    info!("[character] 切换到角色: {} ({})", character.name, agent_id);

    // 触发 WebSocket 重连以切换到新角色
    if let Some(ref tx) = state.reconnect_tx {
        let server_ws_url = state.server_ws_url.read().await.clone();
        let reconnect_req = crate::infra::api::ReconnectRequest {
            ws_url: server_ws_url,
            agent_id: Some(agent_id),
        };
        if let Err(e) = tx.send(reconnect_req) {
            error!("[character] 切换角色后触发重连失败: {}", e);
        } else {
            info!("[character] 切换角色后触发 WebSocket 重连");
        }
    }

    Json(SwitchCharacterResponse {
        success: true,
        message: format!("已切换到角色: {}", character.name),
        character: Some(CharacterInfo {
            agent_id: Some(agent_id.to_string()),
            name: character.name.clone(),
            age: character.age,
            gender: character.gender.clone(),
            appearance: character.appearance.clone(),
            identity: character.identity.clone(),
            personality: character.personality.clone(),
            values: character.values.clone(),
            status: "alive".to_string(),
            server_url: character.server_url.clone(),
            registered_at: character.registered_at.map(|t| t.to_rfc3339()),
            is_current: true,
            last_connected_real_time: character.last_connected_real_time.map(|t| t.to_rfc3339()),
            last_connected_world_time: character
                .last_connected_world_time
                .as_ref()
                .map(|wt| wt.to_chinese()),
        }),
    })
    .into_response()
}

// ============================================================================
