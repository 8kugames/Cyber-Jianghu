// ============================================================================
// 归隐与自动重生（retire / auto-rebirth）
// ============================================================================

use super::*;

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
                // 归隐角色立即退出运行时世界：从状态缓存移除，防止 Tick 边界
                // 继续以更高 tick_id 持久化缓存中的 is_alive=true 快照，
                // 覆盖 retire_agent 写入的 is_alive=false 归隐快照，
                // 导致仪表盘把已归隐角色误报为存活
                if let Some(retired_id) = result.retired_agent_id {
                    state.agent_state_cache.remove(&retired_id);
                }
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
    pub system_prompt: String,
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
/// auto-rebirth 错误响应包装（装箱控制 Result Err 体积，clippy result_large_err；
/// axum 0.8 未提供 Box<T> 的 IntoResponse blanket impl，需本地 newtype 转发）
pub struct AutoRebirthErr(Box<(StatusCode, Json<AutoRebirthResponse>)>);

impl axum::response::IntoResponse for AutoRebirthErr {
    fn into_response(self) -> axum::response::Response {
        (*self.0).into_response()
    }
}

pub async fn agent_auto_rebirth(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<AutoRebirthRequest>,
) -> Result<Json<AutoRebirthResponse>, AutoRebirthErr> {
    info!(
        "自动转世重生请求: old_agent={}, device={}",
        payload.old_agent_id, payload.device_id
    );

    // 前置拦截 nil UUID
    if let Err(e) = db::ensure_old_agent_id_not_nil(payload.old_agent_id) {
        return Err(AutoRebirthErr(Box::new((
            StatusCode::BAD_REQUEST,
            Json(AutoRebirthResponse {
                success: false,
                message: e.to_string(),
                new_agent_id: String::new(),
                old_agent_id: payload.old_agent_id.to_string(),
                spawn_location: String::new(),
                system_prompt: String::new(),
            }),
        ))));
    }

    // 验证设备认证
    let valid = verify_device_token(&state.db_pool, payload.device_id, &payload.auth_token)
        .await
        .map_err(|e| {
            error!("设备认证失败: device_id={}, error={}", payload.device_id, e);
            AutoRebirthErr(Box::new((
                StatusCode::UNAUTHORIZED,
                Json(AutoRebirthResponse {
                    success: false,
                    message: "设备认证失败".to_string(),
                    new_agent_id: String::new(),
                    old_agent_id: payload.old_agent_id.to_string(),
                    spawn_location: String::new(),
                    system_prompt: String::new(),
                }),
            )))
        })?;

    if !valid {
        return Err(AutoRebirthErr(Box::new((
            StatusCode::UNAUTHORIZED,
            Json(AutoRebirthResponse {
                success: false,
                message: "认证令牌无效".to_string(),
                new_agent_id: String::new(),
                old_agent_id: payload.old_agent_id.to_string(),
                spawn_location: String::new(),
                system_prompt: String::new(),
            }),
        ))));
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
        // 重生发放走独立清单（rebirth_items）：死亡不应是"满补给重置"的廉价策略
        let initial_items = game_data::InitialInventoryRegistry::rebirth_items();
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

    // 从内存原子变量取当前世界 tick；未启动 scheduler 时回退到 DB。
    // 之前 auto_rebirth_agent 内部用 `MAX(agent_states.tick_id) WHERE agent_id = old + 1`
    // 会在"死亡到重生之间世界已推进 N tick"时让新角色 state 落后世界 N tick。
    let world_tick = {
        let live = state
            .current_accepting_tick_id
            .load(std::sync::atomic::Ordering::Acquire);
        if live > 0 {
            live
        } else {
            db::get_current_world_tick_id(&state.db_pool)
                .await
                .unwrap_or(0)
        }
    };

    // 执行转世重生（单事务）
    let result = db::auto_rebirth_agent(
        &state.db_pool,
        payload.old_agent_id,
        payload.device_id, // 传入 caller device_id，DB 层强制归属校验
        db::AutoRebirthParams {
            spawn_location: &spawn_location,
            initial_items: &initial_items_data,
            starting_age_ticks,
            reset_recipes,
            world_tick,
        },
    )
    .await
    .map_err(|e| {
        error!(
            "转世重生失败: old_agent={}, error={}",
            payload.old_agent_id, e
        );
        AutoRebirthErr(Box::new((
            StatusCode::BAD_REQUEST,
            Json(AutoRebirthResponse {
                success: false,
                message: format!("转世重生失败: {}", e),
                new_agent_id: String::new(),
                old_agent_id: payload.old_agent_id.to_string(),
                spawn_location: String::new(),
                system_prompt: String::new(),
            }),
        )))
    })?;

    // 不在 HTTP handler 中旁路改写内存态。
    // 新角色的 state_cache / agent_to_device_map 由 DB + WebSocket 重连路径恢复，
    // 避免与 IntentWorker 的单写模型形成代际竞态。

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
        system_prompt: result.system_prompt,
    }))
}
