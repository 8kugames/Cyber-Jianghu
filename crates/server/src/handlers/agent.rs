use anyhow::Result;
use axum::{Json, extract::State, http::StatusCode};
use std::sync::Arc;
use tracing::{error, info};

use crate::db::{self, DeviceConnectResult, verify_device_token};
use crate::game_data;
use crate::models::{
    AgentConnectRequest, AgentConnectResponse, AgentRegisterRequest, AgentRegisterResponse,
    GameRules, InitialItem, get_max_agent_name_length, get_max_system_prompt_length,
};
use crate::state::AppState;

// ============================================================================
// 设备连接 API（Phase 3）
// ============================================================================

/// 设备连接接口
///
/// POST /api/v1/agent/connect
///
/// 客户端首次启动时调用，用于注册设备身份或获取现有认证令牌。
///
/// 流程：
/// 1. 客户端生成 device_id (UUID v4)
/// 2. 调用此接口注册设备
/// 3. 服务器返回 auth_token
/// 4. 客户端保存 device_id + auth_token 到 agent.yaml
///
/// 后续 WebSocket 连接使用: ws://server/ws?device_id={}&token={}
pub async fn agent_connect(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<AgentConnectRequest>,
) -> Result<Json<AgentConnectResponse>, StatusCode> {
    info!("设备连接请求: {}", payload.device_id);

    // 注册或获取设备
    let DeviceConnectResult {
        device_id,
        auth_token,
        is_new,
    } = db::connect_device(&state.db_pool, payload.device_id)
        .await
        .map_err(|e| {
            error!("设备连接失败: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let message = if is_new {
        format!("设备 {} 注册成功", device_id)
    } else {
        format!("设备 {} 已连接", device_id)
    };

    info!("{}", message);

    Ok(Json(AgentConnectResponse {
        auth_token,
        message,
    }))
}

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

    // 4. 获取当前服务器的 tick_id（避免新 agent 累积"出生前"伤害）
    let current_tick_id = crate::db::get_current_world_tick_id(&state.db_pool)
        .await
        .unwrap_or(0);

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
        reflector_narrative: None,
        immediate_events,
        lifespan,
        calendar: crate::game_data::registry::TimeRegistry::get_config().map(|tc| {
            cyber_jianghu_protocol::CalendarConfig {
                days_per_season: tc.days_per_season as u32,
                seasons_per_year: tc.seasons_per_year as u32,
            }
        }),
        daily_summary: None,
    };

    // 8. 获取叙事化配置（用于属性描述转换）
    let narrative_config = state.game_data.get().narrative.clone();

    // 9. 获取初始属性（先天属性，用于 Agent 端存储 birth_attributes）
    let initial_attributes = registration.initial_state.get_attributes_for_protocol();

    Ok(Json(AgentRegisterResponse {
        agent_id: agent.agent_id.to_string(),
        message: format!("Agent '{}' registered successfully", agent.name),
        game_rules,
        narrative_config,
        initial_attributes,
    }))
}

// ============================================================================
// Agent 转生 API（Phase 4 - 归隐重生）
// ============================================================================

/// 转生请求
#[derive(Debug, serde::Deserialize)]
pub struct RebirthRequest {
    /// 设备 ID
    pub device_id: uuid::Uuid,
    /// 认证令牌
    pub auth_token: String,
}

/// 转生响应
#[derive(Debug, serde::Serialize)]
pub struct RebirthResponse {
    /// 是否成功
    pub success: bool,
    /// 消息
    pub message: String,
    /// 归隐的角色 ID
    pub retired_agent_id: Option<String>,
}

/// Agent 转生接口
///
/// POST /api/v1/agent/rebirth
///
/// 将当前设备的角色标记为归隐状态，保留设备身份和历史数据，允许重新创建新角色。
/// - 角色状态从 active 变为 retired
/// - 保留所有历史数据（agent_states, agent_inventory）
/// - 可通过 Web 面板查看历史角色
pub async fn agent_rebirth(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<RebirthRequest>,
) -> Result<Json<RebirthResponse>, (StatusCode, Json<RebirthResponse>)> {
    info!("Agent 转生请求: device_id={}", payload.device_id);

    // 调用数据库操作
    match db::rebirth_agent(&state.db_pool, payload.device_id, &payload.auth_token).await {
        Ok(result) => {
            info!(
                "Agent 转生成功: {} ({}) 已归隐",
                result.retired_name, result.retired_agent_id
            );
            Ok(Json(RebirthResponse {
                success: true,
                message: format!("角色 '{}' 已归隐，可以创建新角色", result.retired_name),
                retired_agent_id: Some(result.retired_agent_id.to_string()),
            }))
        }
        Err(e) => {
            let error_msg = format!("{}", e);
            error!("Agent 转生失败: {}", error_msg);

            // 根据错误类型确定状态码
            let status = if error_msg.contains("认证失败") || error_msg.contains("auth") {
                StatusCode::UNAUTHORIZED
            } else if error_msg.contains("没有活跃的角色") || error_msg.contains("无需归隐")
            {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_REQUEST
            };

            Err((
                status,
                Json(RebirthResponse {
                    success: false,
                    message: error_msg,
                    retired_agent_id: None,
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
/// 服务端在单一事务中：创建全新 agent_id + 初始状态 + 初始物品。旧 agent 保持 dead 状态。
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

    // 执行转世重生（单事务）
    let result = db::auto_rebirth_agent(
        &state.db_pool,
        payload.old_agent_id,
        &spawn_location,
        true,
        &initial_items_data,
        starting_age_ticks,
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

    // 更新 DashMap（内存缓存）— 旧条目在 death 时已移除
    let new_state = crate::models::AgentState::new(
        result.agent_id,
        crate::db::get_current_world_tick_id(&state.db_pool)
            .await
            .unwrap_or(0),
    );
    state
        .agent_state_cache
        .insert(result.agent_id, new_state);

    // 更新 agent_to_device_map
    {
        let mut map = state.agent_to_device_map.write().await;
        map.insert(result.agent_id, payload.device_id);
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
