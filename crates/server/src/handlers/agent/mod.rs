use anyhow::Result;
use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
    http::StatusCode,
};
use std::sync::Arc;
use tracing::{error, info};

use crate::db::{self, verify_device_token};
use crate::game_data;
use crate::models::{
    AgentRegisterRequest, AgentRegisterResponse, GameRules, InitialItem, get_max_agent_name_length,
    get_max_system_prompt_length,
};
use crate::state::AppState;

// ============================================================================
// Agent 注册 API（角色创建）
// ============================================================================

/// Agent降生注册接口
///
/// POST /api/v1/agent/register
///
/// 实现Agent注册流程（事务性）：
/// 1. 验证设备认证（device_id + auth_token）
/// 2. 验证 name 和 system_prompt
/// 3. 在单个事务中执行：
///    - 创建Agent记录
///    - 创建初始状态（使用当前 tick_id）
///    - 分配默认初始物品
/// 4. 构建并返回游戏规则
///
/// 注意：system_prompt 由服务器根据 payload 字段统一生成
/// （`payload.generate_system_prompt()`），而非直接接受客户端提交的 prompt 字符串。
/// 这是从协议层根治 prompt injection——客户端无法注入"忽略所有指令"等覆盖性内容。
pub async fn agent_register(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<AgentRegisterRequest>,
) -> Result<Json<AgentRegisterResponse>, StatusCode> {
    info!("Agent registration request: {}", payload.name);

    // 1. 验证设备认证（device_id + auth_token）
    let device_valid = verify_device_token(&state.db_pool, payload.device_id, &payload.auth_token)
        .await
        .map_err(|e| {
            error!("Device verification failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if !device_valid {
        error!(
            "Invalid device credentials: device_id={}",
            payload.device_id
        );
        return Err(StatusCode::UNAUTHORIZED);
    }

    // 2. 验证 name 长度
    if payload.name.is_empty() || payload.name.len() > get_max_agent_name_length() {
        error!("Invalid name length: {} chars", payload.name.len());
        return Err(StatusCode::BAD_REQUEST);
    }

    // 3. 强制由服务端统一生成 system_prompt，防御客户端注入
    let system_prompt = payload.generate_system_prompt();

    // 验证 system_prompt 长度，防止构造过长的属性导致截断或攻击
    if system_prompt.is_empty() || system_prompt.len() > get_max_system_prompt_length() {
        error!(
            "Generated system_prompt length exceeds limit: {} bytes",
            system_prompt.len()
        );
        return Err(StatusCode::BAD_REQUEST);
    }

    // 4. 获取当前服务器的 tick_id
    // 优先使用 scheduler 实时计算的 tick_id（Arc<AtomicI64>），
    // 仅在 scheduler 未启动时 fallback 到 DB 查询。
    let current_tick_id = {
        let live_tick = state
            .current_accepting_tick_id
            .load(std::sync::atomic::Ordering::Acquire);
        if live_tick > 0 {
            live_tick
        } else {
            crate::db::get_current_world_tick_id(&state.db_pool)
                .await
                .unwrap_or(0)
        }
    };

    // 5. 准备初始物品数据
    let initial_items = game_data::InitialInventoryRegistry::items();
    let initial_items_data: Vec<(String, String, i32, String)> = initial_items
        .iter()
        .map(|item| {
            (
                item.item_id.clone(),
                item.name.clone(),
                item.quantity,
                item.description.clone(),
            )
        })
        .collect();

    // 6. 事务性注册（原子性保证）
    let registration = match crate::db::register_agent_transactional(
        &state.db_pool,
        payload.device_id, // 关联设备ID
        &payload.name,
        &system_prompt,
        current_tick_id,
        &initial_items_data,
        payload.model_id.as_deref(),
    )
    .await
    {
        Ok(reg) => reg,
        Err(e) => {
            error!("Agent registration transaction failed: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let agent = registration.agent;
    info!(
        "Agent '{}' registered successfully (transactional)!",
        agent.name
    );

    // 6.5 分配初始配方（根据角色名匹配 initial_recipes.yaml）
    {
        let initial_recipes =
            crate::game_data::registry::InitialRecipesRegistry::get_initial_recipes(Some(
                &agent.name,
            ));
        if !initial_recipes.is_empty() {
            if let Err(e) = crate::db::assign_initial_recipes(
                &state.db_pool,
                agent.agent_id,
                &initial_recipes,
                current_tick_id,
            )
            .await
            {
                tracing::warn!("Failed to assign initial recipes for {}: {}", agent.name, e);
            } else {
                tracing::info!(
                    "Assigned {} initial recipes to {}",
                    initial_recipes.len(),
                    agent.name
                );
            }
        }
    }

    // 7.5 更新 agent_id → device_id 反向映射（用于 WebSocket 广播）
    {
        let mut agent_to_device = state.agent_to_device_map.write().await;
        agent_to_device.insert(agent.agent_id, payload.device_id);
        info!(
            "Updated agent_to_device_map: {} → {}",
            agent.agent_id, payload.device_id
        );
    }

    // 7. 构建游戏规则（从配置动态获取）
    let (
        tick_duration_secs,
        survival,
        game_rules_version,
        immediate_events,
        intent_batch,
        lifespan,
        dialogue_context,
    ) = {
        let gd = state.game_data.get();
        (
            gd.game_rules.data.agent_state.tick.real_seconds_per_tick as u64,
            crate::websocket::types::SurvivalConfig {
                rebirth_delay_ticks: gd.game_rules.data.agent_state.survival.rebirth.delay_ticks,
                rebirth_retry_max_attempts: gd
                    .game_rules
                    .data
                    .agent_state
                    .survival
                    .rebirth
                    .retry_max_attempts,
                rebirth_retry_interval_secs: gd
                    .game_rules
                    .data
                    .agent_state
                    .survival
                    .rebirth
                    .retry_interval_secs,
            },
            gd.game_rules.version.clone(),
            gd.game_rules.data.immediate_events.clone(),
            gd.game_rules.data.intent_batch.clone(),
            gd.game_rules.data.lifespan.clone(),
            gd.game_rules.data.dialogue_context.clone(),
        )
    };
    let game_rules = GameRules {
        tick_duration_secs,
        available_actions: game_data::ActionRegistry::build_available_actions(),
        initial_items: initial_items
            .into_iter()
            .map(|item| InitialItem {
                item_id: item.item_id,
                name: item.name,
                quantity: item.quantity,
                description: item.description,
            })
            .collect(),
        survival_actions: game_data::ActionRegistry::action_names_with_tag("survival"),
        version: game_rules_version,
        last_updated: chrono::Utc::now().to_rfc3339(),
        intent_batch,
        rebirth_delay_ticks: survival.rebirth_delay_ticks,
        rebirth_retry_max_attempts: survival.rebirth_retry_max_attempts,
        rebirth_retry_interval_secs: survival.rebirth_retry_interval_secs,
        immediate_events,
        lifespan,
        calendar: crate::game_data::registry::TimeRegistry::get_config().map(|tc| {
            cyber_jianghu_protocol::CalendarConfig {
                days_per_season: tc.days_per_season as u32,
                seasons_per_year: tc.seasons_per_year as u32,
            }
        }),
        daily_summary: None,
        dialogue_context,
    };

    // 8. 获取叙事化配置（用于属性描述转换）
    let narrative_config = state.game_data.get().narrative.clone();
    let nc_hash = cyber_jianghu_protocol::payload_hash(&narrative_config);

    // 9. 获取初始属性（先天属性，用于 Agent 端存储 birth_attributes）
    let initial_attributes = registration.initial_state.get_attributes_for_protocol();

    Ok(Json(AgentRegisterResponse {
        agent_id: agent.agent_id.to_string(),
        message: format!("Agent '{}' registered successfully", agent.name),
        game_rules,
        narrative_config,
        narrative_config_hash: nc_hash,
        system_prompt,
        initial_attributes,
    }))
}

// ============================================================================
// 传记回传 API
// ============================================================================

#[derive(serde::Deserialize)]
pub struct BiographyRequest {
    pub agent_id: uuid::Uuid,
    pub biography: String,
}

/// POST /api/v1/agent/biography
///
/// Agent 端在角色死亡/归隐时调用，将 LLM 生成的纪传体传记回传到 server
pub async fn update_biography(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<BiographyRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if payload.biography.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "biography must not be empty"})),
        ));
    }

    match db::update_agent_biography(&state.db_pool, payload.agent_id, &payload.biography).await {
        Ok(()) => {
            info!("[biography] 传记已保存: agent={}", payload.agent_id);
            Ok(Json(serde_json::json!({"success": true})))
        }
        Err(e) => {
            error!(
                "[biography] 传记保存失败: agent={}, err={}",
                payload.agent_id, e
            );
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("保存失败: {}", e)})),
            ))
        }
    }
}

/// GET /api/v1/agent/{id}/biography
///
/// 从数据库查询角色传记，供 agent 端回退读取（agent 本地 character.yaml 无传记时使用）
pub async fn get_agent_biography(
    State(state): State<Arc<AppState>>,
    Path(agent_id): Path<uuid::Uuid>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let biography: Option<String> =
        sqlx::query_scalar("SELECT biography FROM agents WHERE agent_id = $1")
            .bind(agent_id)
            .fetch_optional(&state.db_pool)
            .await
            .map_err(|e| {
                error!("[biography] 查询失败: agent={}, err={}", agent_id, e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": "数据库查询失败"})),
                )
            })?;

    match biography {
        Some(bio) if !bio.is_empty() => Ok(Json(serde_json::json!({"biography": bio}))),
        _ => Ok(Json(serde_json::json!({"biography": null}))),
    }
}

// ============================================================================
// Prompt Templates 获取（Agent 启动时主动拉取）
// ============================================================================

#[derive(Debug, serde::Deserialize)]
pub struct GetPromptTemplatesRequest {
    pub device_id: uuid::Uuid,
    pub auth_token: String,
}

#[derive(Debug, serde::Serialize)]
pub struct PromptTemplatesResponse {
    pub hash: String,
    pub version: String,
    pub content: serde_json::Value,
}

/// POST /api/v1/agent/prompt-templates
///
/// Agent 启动时主动拉取 prompt_templates JSON。
/// 使用 device token 认证（与 agent_register / agent_retire 一致）。
pub async fn get_prompt_templates(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<GetPromptTemplatesRequest>,
) -> Result<Json<PromptTemplatesResponse>, (StatusCode, Json<PromptTemplatesResponse>)> {
    let valid = verify_device_token(&state.db_pool, payload.device_id, &payload.auth_token)
        .await
        .map_err(|e| {
            tracing::warn!(
                "prompt-templates 设备认证失败: device_id={}, error={}",
                payload.device_id,
                e
            );
            (
                StatusCode::UNAUTHORIZED,
                Json(PromptTemplatesResponse {
                    hash: String::new(),
                    version: String::new(),
                    content: serde_json::Value::Null,
                }),
            )
        })?;

    if !valid {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(PromptTemplatesResponse {
                hash: String::new(),
                version: String::new(),
                content: serde_json::Value::Null,
            }),
        ));
    }

    let cache = state.prompt_template_cache.read().await;
    match cache.as_ref() {
        Some(pt_cache) => Ok(Json(PromptTemplatesResponse {
            hash: pt_cache.hash.clone(),
            version: pt_cache.version.clone(),
            content: pt_cache.json_value.clone(),
        })),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(PromptTemplatesResponse {
                hash: String::new(),
                version: String::new(),
                content: serde_json::Value::Null,
            }),
        )),
    }
}

mod grant;
mod rebirth;

pub use grant::{agent_grant_items, agent_grant_recipes};
pub use rebirth::{agent_auto_rebirth, agent_retire};
