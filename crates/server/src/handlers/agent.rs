use anyhow::Result;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use sha2::Digest;
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
// Agent 注册 API（Phase 4 - 角色创建）
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
/// 注意：根据架构原则，服务器只负责世界状态和规则。
/// Agent人设Prompt应由客户端（Agent SDK）提供，服务器仅存储。
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

    // 3. 获取并验证 system_prompt（必需）
    let system_prompt = payload.system_prompt.ok_or_else(|| {
        error!("Missing required field: system_prompt");
        StatusCode::BAD_REQUEST
    })?;

    // 验证 system_prompt 长度，防止滥用
    if system_prompt.is_empty() || system_prompt.len() > get_max_system_prompt_length() {
        error!(
            "Invalid system_prompt length: {} bytes",
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

    // 6. 事务性注册（F-04：原子性保证）
    let registration = match crate::db::register_agent_transactional(
        &state.db_pool,
        payload.device_id, // 关联设备ID
        &payload.name,
        &system_prompt,
        current_tick_id,
        &initial_items_data,
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
    let nc_hash = serde_json::to_vec(&narrative_config)
        .ok()
        .map(|bytes| format!("{:x}", sha2::Sha256::digest(&bytes)));

    // 9. 获取初始属性（先天属性，用于 Agent 端存储 birth_attributes）
    let initial_attributes = registration.initial_state.get_attributes_for_protocol();

    Ok(Json(AgentRegisterResponse {
        agent_id: agent.agent_id.to_string(),
        message: format!("Agent '{}' registered successfully", agent.name),
        game_rules,
        narrative_config,
        narrative_config_hash: nc_hash,
        initial_attributes,
    }))
}

// ============================================================================
// Agent 归隐 API
// ============================================================================

/// 归隐请求
#[derive(Debug, serde::Deserialize)]
pub struct RetireRequest {
    /// 设备 ID
    pub device_id: uuid::Uuid,
    /// 认证令牌
    pub auth_token: String,
}

/// 归隐响应
#[derive(Debug, serde::Serialize)]
pub struct RetireResponse {
    /// 是否成功
    pub success: bool,
    /// 消息
    pub message: String,
    /// 归隐的角色 ID
    pub retired_agent_id: Option<String>,
    /// 是否执行了归隐操作（false = 角色已是 dead/retired 终态）
    pub action_taken: bool,
}

/// Agent 归隐接口
///
/// POST /api/v1/agent/retire
///
/// 幂等操作：将当前设备的活跃角色标记为归隐状态。
/// 如果角色已是 dead/retired 终态，返回成功但 action_taken=false。
pub async fn agent_retire(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<RetireRequest>,
) -> Result<Json<RetireResponse>, (StatusCode, Json<RetireResponse>)> {
    info!("Agent 归隐请求: device_id={}", payload.device_id);

    match db::retire_agent(&state.db_pool, payload.device_id, &payload.auth_token).await {
        Ok(result) => {
            if result.action_taken {
                info!(
                    "Agent 归隐成功: {} ({}) 已归隐",
                    result.retired_name.as_ref().unwrap_or(&"-".to_string()),
                    result
                        .retired_agent_id
                        .map(|id| id.to_string())
                        .unwrap_or_default()
                );
                Ok(Json(RetireResponse {
                    success: true,
                    message: format!(
                        "角色 '{}' 已归隐，可以创建新角色",
                        result.retired_name.as_ref().unwrap_or(&"-".to_string())
                    ),
                    retired_agent_id: result.retired_agent_id.map(|id| id.to_string()),
                    action_taken: true,
                }))
            } else {
                info!("Agent 归隐：无活跃角色需要归隐");
                Ok(Json(RetireResponse {
                    success: true,
                    message: "无活跃角色需要归隐".to_string(),
                    retired_agent_id: None,
                    action_taken: false,
                }))
            }
        }
        Err(e) => {
            let error_msg = format!("{}", e);
            error!("Agent 归隐失败: {}", error_msg);

            let status = if error_msg.contains("认证失败") || error_msg.contains("auth") {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::BAD_REQUEST
            };

            Err((
                status,
                Json(RetireResponse {
                    success: false,
                    message: error_msg,
                    retired_agent_id: None,
                    action_taken: false,
                }),
            ))
        }
    }
}

// ============================================================================
// Agent 自动重生 API（转世：dead → retired + new agent_id）
// ============================================================================

/// 自动重生请求（转世）
#[derive(Debug, serde::Deserialize)]
pub struct AutoRebirthRequest {
    /// 设备 ID（连接身份）
    pub device_id: uuid::Uuid,
    /// 认证令牌
    pub auth_token: String,
    /// 旧 Agent ID（已死亡的角色）
    pub old_agent_id: uuid::Uuid,
    /// 角色名称
    pub name: String,
    /// 系统提示词
    pub system_prompt: String,
}

/// 自动重生响应（转世）
#[derive(Debug, serde::Serialize)]
pub struct AutoRebirthResponse {
    pub success: bool,
    pub message: String,
    /// 新 Agent ID
    pub new_agent_id: String,
    /// 旧 Agent ID（已 retired）
    pub old_agent_id: String,
    pub spawn_location: String,
}

/// Agent 自动重生接口（转世）
///
/// POST /api/v1/agent/auto-rebirth
///
/// Agent 端在等待 rebirth_delay_ticks 后调用此接口完成转世重生。
/// 服务端在单一事务中：创建全新 agent_id + 初始状态 + 初始物品。
///
/// 旧 agent 终态：保持 `status='dead'` 死亡标记，`retired_at` 字段作为时间戳记录转世完成事件。
/// `retired` 状态不被 auto-rebirth 触及（仅 `/api/v1/agent/retire` 端点可设置，专属"玩家主动归隐"语义）。
pub async fn agent_auto_rebirth(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<AutoRebirthRequest>,
) -> Result<Json<AutoRebirthResponse>, (StatusCode, Json<AutoRebirthResponse>)> {
    info!(
        "自动转世重生请求: old_agent={}, device={}",
        payload.old_agent_id, payload.device_id
    );

    // 验证设备认证
    let valid = verify_device_token(&state.db_pool, payload.device_id, &payload.auth_token)
        .await
        .map_err(|e| {
            error!("设备认证失败: device_id={}, error={}", payload.device_id, e);
            (
                StatusCode::UNAUTHORIZED,
                Json(AutoRebirthResponse {
                    success: false,
                    message: "设备认证失败".to_string(),
                    new_agent_id: String::new(),
                    old_agent_id: payload.old_agent_id.to_string(),
                    spawn_location: String::new(),
                }),
            )
        })?;

    if !valid {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(AutoRebirthResponse {
                success: false,
                message: "认证令牌无效".to_string(),
                new_agent_id: String::new(),
                old_agent_id: payload.old_agent_id.to_string(),
                spawn_location: String::new(),
            }),
        ));
    }

    // 从配置读取重生参数
    let (spawn_location, initial_items_data) = {
        let gd = state.game_data.get();
        let rebirth_config = &gd.game_rules.data.agent_state.survival.rebirth;
        let spawn_location = if rebirth_config.spawn_location.is_empty() {
            gd.game_rules
                .data
                .agent_state
                .location
                .spawn_location
                .clone()
        } else {
            rebirth_config.spawn_location.clone()
        };
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
        (spawn_location, initial_items_data)
    };

    let starting_age_ticks = crate::tick::decay::compute_starting_age_ticks();

    // 读取重生配置
    let reset_recipes = crate::game_data::registry()
        .map(|cache| {
            cache
                .get()
                .game_rules
                .data
                .agent_state
                .survival
                .rebirth
                .reset_recipes
        })
        .unwrap_or(true);

    // 执行转世重生（单事务）
    let result = db::auto_rebirth_agent(
        &state.db_pool,
        payload.old_agent_id,
        &spawn_location,
        &initial_items_data,
        starting_age_ticks,
        reset_recipes,
    )
    .await
    .map_err(|e| {
        error!(
            "转世重生失败: old_agent={}, error={}",
            payload.old_agent_id, e
        );
        (
            StatusCode::BAD_REQUEST,
            Json(AutoRebirthResponse {
                success: false,
                message: format!("转世重生失败: {}", e),
                new_agent_id: String::new(),
                old_agent_id: payload.old_agent_id.to_string(),
                spawn_location: String::new(),
            }),
        )
    })?;

    // 更新 DashMap（内存缓存）— 移除旧 agent 缓存
    state.agent_state_cache.remove(&payload.old_agent_id);
    let new_state = crate::models::AgentState::new(
        result.agent_id,
        crate::db::get_current_world_tick_id(&state.db_pool)
            .await
            .unwrap_or(0),
    );
    state.agent_state_cache.insert(result.agent_id, new_state);

    // 更新 agent_to_device_map（清理旧映射 + 建立新映射）
    {
        let mut map = state.agent_to_device_map.write().await;
        map.remove(&payload.old_agent_id);
        map.insert(result.agent_id, payload.device_id);
    }

    // 重生后重新分配初始配方
    {
        let initial_recipes =
            crate::game_data::registry::InitialRecipesRegistry::get_initial_recipes(Some(
                &result.name,
            ));
        if !initial_recipes.is_empty() {
            let tick_id = crate::db::get_current_world_tick_id(&state.db_pool)
                .await
                .unwrap_or(0);
            if let Err(e) = crate::db::assign_initial_recipes(
                &state.db_pool,
                result.agent_id,
                &initial_recipes,
                tick_id,
            )
            .await
            {
                tracing::warn!("Rebirth recipe assignment failed: {}", e);
            }
        }
    }

    info!(
        "Agent 转世重生成功: agent={}, name={}, spawn={}",
        result.agent_id, result.name, result.spawn_location
    );

    Ok(Json(AutoRebirthResponse {
        success: true,
        message: format!(
            "角色 '{}' 已转世重生到 {}",
            result.name, result.spawn_location
        ),
        new_agent_id: result.agent_id.to_string(),
        old_agent_id: payload.old_agent_id.to_string(),
        spawn_location: result.spawn_location,
    }))
}

// ============================================================================
// 管理员库存注入 API（Vendor 支持）
// ============================================================================

/// 库存注入请求
#[derive(Debug, serde::Deserialize)]
pub struct GrantItemsRequest {
    /// Agent ID
    pub agent_id: uuid::Uuid,
    /// 物品列表 (item_id, quantity)
    pub items: Vec<GrantItem>,
}

/// 单个物品
#[derive(Debug, serde::Deserialize)]
pub struct GrantItem {
    pub item_id: String,
    pub quantity: i32,
}

/// 库存注入响应
#[derive(Debug, serde::Serialize)]
pub struct GrantItemsResponse {
    pub success: bool,
    pub message: String,
    pub granted_count: usize,
}

/// 管理员库存注入接口
///
/// POST /api/v1/agent/grant-items
///
/// 为指定 Agent 注入物品库存（用于 Vendor 补货等管理操作）。
pub async fn agent_grant_items(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<GrantItemsRequest>,
) -> Result<Json<GrantItemsResponse>, (StatusCode, Json<GrantItemsResponse>)> {
    if payload.items.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(GrantItemsResponse {
                success: false,
                message: "物品列表为空".to_string(),
                granted_count: 0,
            }),
        ));
    }

    // 验证每个物品：存在性 + 数量合法性
    for item in &payload.items {
        if !crate::game_data::registry::ItemRegistry::exists(&item.item_id) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(GrantItemsResponse {
                    success: false,
                    message: format!("物品 '{}' 不存在", item.item_id),
                    granted_count: 0,
                }),
            ));
        }
        if item.quantity <= 0 || item.quantity > 9999 {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(GrantItemsResponse {
                    success: false,
                    message: format!("物品 '{}' 数量不合法 (1-9999)", item.item_id),
                    granted_count: 0,
                }),
            ));
        }
    }

    let mut granted = 0usize;
    for item in &payload.items {
        // 直接 INSERT ... ON CONFLICT DO UPDATE 实现叠加
        let result = sqlx::query(
            r#"
            INSERT INTO agent_inventory (agent_id, item_id, quantity)
            VALUES ($1, $2, $3)
            ON CONFLICT (agent_id, item_id)
            DO UPDATE SET
                quantity = agent_inventory.quantity + EXCLUDED.quantity,
                updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(payload.agent_id)
        .bind(&item.item_id)
        .bind(item.quantity)
        .execute(&state.db_pool)
        .await;

        match result {
            Ok(_) => {
                info!(
                    "Grant: agent={}, item={}, qty={}",
                    payload.agent_id, item.item_id, item.quantity
                );
                granted += 1;
            }
            Err(e) => {
                error!(
                    "Grant failed: agent={}, item={}, error={}",
                    payload.agent_id, item.item_id, e
                );
            }
        }
    }

    info!(
        "管理员库存注入完成: agent={}, granted={}/{}",
        payload.agent_id,
        granted,
        payload.items.len()
    );

    // 注入 LLM 消息（"意外获得......，可用于销售"）
    if granted > 0 {
        let items_desc: String = payload
            .items
            .iter()
            .map(|i| {
                let name = crate::game_data::registry::ItemRegistry::get(&i.item_id)
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| i.item_id.clone());
                format!("{}×{}", name, i.quantity)
            })
            .collect::<Vec<_>>()
            .join("、");

        let event = crate::models::WorldEvent {
            event_type: cyber_jianghu_protocol::WorldEventType::SystemNotification,
            tick_id: 0,
            description: format!("意外获得{}，可用于销售", items_desc),
            metadata: serde_json::json!({
                "type": "vendor_grant",
                "items": payload.items.iter().map(|i| serde_json::json!({"item_id": i.item_id, "quantity": i.quantity})).collect::<Vec<_>>(),
            }),
        };
        state
            .vendor_pending_events
            .entry(payload.agent_id)
            .or_default()
            .push(event);
    }

    Ok(Json(GrantItemsResponse {
        success: granted > 0,
        message: format!("成功注入 {} 个物品", granted),
        granted_count: granted,
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
