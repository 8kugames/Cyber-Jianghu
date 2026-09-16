// ============================================================================
// 管理员物品/配方注入（grant-items / grant-recipes，Vendor 支持）
// ============================================================================

use super::*;

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

#[derive(Debug, sqlx::FromRow)]
struct AuditInventorySnapshotRow {
    item_id: String,
    quantity: i32,
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
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<GrantItemsRequest>,
) -> Result<Json<GrantItemsResponse>, (StatusCode, Json<GrantItemsResponse>)> {
    let audit_ctx = crate::db::build_audit_request_context(&headers, addr);
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

    let before_state = sqlx::query_as::<_, AuditInventorySnapshotRow>(
        "SELECT item_id, quantity FROM agent_inventory WHERE agent_id = $1 ORDER BY item_id ASC",
    )
    .bind(payload.agent_id)
    .fetch_all(&state.db_pool)
    .await
    .ok()
    .map(|items| {
        serde_json::json!(
            items
                .into_iter()
                .map(|item| serde_json::json!({"item_id": item.item_id, "quantity": item.quantity}))
                .collect::<Vec<_>>()
        )
    });

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

    if granted > 0
        && let Err(e) = crate::db::insert_audit_log(
            &state.db_pool,
            crate::db::AuditLogEntry {
                event_type: "agent.grant_items",
                actor_type: "admin",
                token_type: Some("write"),
                resource_type: "agent_inventory",
                resource_id: Some(payload.agent_id.to_string()),
                endpoint: "/api/v1/agent/grant-items",
                method: "POST",
                result: "success",
                reason: None,
                payload: serde_json::json!({
                    "agent_id": payload.agent_id,
                    "granted_count": granted,
                    "items": payload.items.iter().map(|item| serde_json::json!({
                        "item_id": item.item_id,
                        "quantity": item.quantity,
                    })).collect::<Vec<_>>(),
                }),
                request_id: Some(audit_ctx.request_id),
                ip: audit_ctx.ip,
                user_agent: audit_ctx.user_agent,
                before_state,
                after_state: Some(serde_json::json!(
                    payload
                        .items
                        .iter()
                        .map(|item| serde_json::json!({
                            "item_id": item.item_id,
                            "quantity_delta": item.quantity,
                        }))
                        .collect::<Vec<_>>()
                )),
            },
        )
        .await
    {
        error!("audit_log 写入失败(agent.grant_items): {}", e);
    }

    Ok(Json(GrantItemsResponse {
        success: granted > 0,
        message: format!("成功注入 {} 个物品", granted),
        granted_count: granted,
    }))
}

/// 配方注入请求
#[derive(Debug, serde::Deserialize)]
pub struct GrantRecipesRequest {
    /// Agent ID
    pub agent_id: uuid::Uuid,
    /// 配方 ID 列表
    pub recipe_ids: Vec<String>,
}

/// 配方注入响应
#[derive(Debug, serde::Serialize)]
pub struct GrantRecipesResponse {
    pub success: bool,
    pub message: String,
    pub granted_count: usize,
}

/// 管理员配方注入接口
///
/// POST /api/v1/agent/grant-recipes
///
/// 为指定 Agent 注入已知配方（制造/传授的前提）。
pub async fn agent_grant_recipes(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<GrantRecipesRequest>,
) -> Result<Json<GrantRecipesResponse>, (StatusCode, Json<GrantRecipesResponse>)> {
    let audit_ctx = crate::db::build_audit_request_context(&headers, addr);
    if payload.recipe_ids.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(GrantRecipesResponse {
                success: false,
                message: "配方列表为空".to_string(),
                granted_count: 0,
            }),
        ));
    }

    // 验证每个配方存在性
    for recipe_id in &payload.recipe_ids {
        if crate::game_data::registry::RecipeRegistry::get(recipe_id).is_none() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(GrantRecipesResponse {
                    success: false,
                    message: format!("配方 '{}' 不存在", recipe_id),
                    granted_count: 0,
                }),
            ));
        }
    }

    let current_tick = crate::db::get_current_world_tick_id(&state.db_pool)
        .await
        .unwrap_or(0);

    let mut granted = 0usize;
    for recipe_id in &payload.recipe_ids {
        let result = sqlx::query(
            r#"
            INSERT INTO agent_known_recipes (agent_id, recipe_id, learned_at_tick, source)
            VALUES ($1, $2, $3, 'admin')
            ON CONFLICT (agent_id, recipe_id) DO NOTHING
            "#,
        )
        .bind(payload.agent_id)
        .bind(recipe_id)
        .bind(current_tick)
        .execute(&state.db_pool)
        .await;

        match result {
            Ok(_) => {
                info!(
                    "Grant recipe: agent={}, recipe={}",
                    payload.agent_id, recipe_id
                );
                granted += 1;
            }
            Err(e) => {
                error!(
                    "Grant recipe failed: agent={}, recipe={}, error={}",
                    payload.agent_id, recipe_id, e
                );
            }
        }
    }

    // 全部已习得不算失败；部分已习得在消息中说明（ON CONFLICT DO NOTHING 幂等）
    let message = if granted == 0 {
        "所选配方均已习得，无需注入".to_string()
    } else if granted < payload.recipe_ids.len() {
        format!("成功注入 {} 个配方（其余已习得）", granted)
    } else {
        format!("成功注入 {} 个配方", granted)
    };

    info!(
        "管理员配方注入完成: agent={}, granted={}/{}",
        payload.agent_id,
        granted,
        payload.recipe_ids.len()
    );

    if granted > 0
        && let Err(e) = crate::db::insert_audit_log(
            &state.db_pool,
            crate::db::AuditLogEntry {
                event_type: "agent.grant_recipes",
                actor_type: "admin",
                token_type: Some("write"),
                resource_type: "agent_known_recipes",
                resource_id: Some(payload.agent_id.to_string()),
                endpoint: "/api/v1/agent/grant-recipes",
                method: "POST",
                result: "success",
                reason: None,
                payload: serde_json::json!({
                    "agent_id": payload.agent_id,
                    "granted_count": granted,
                    "recipe_ids": payload.recipe_ids,
                }),
                request_id: Some(audit_ctx.request_id),
                ip: audit_ctx.ip,
                user_agent: audit_ctx.user_agent,
                before_state: None,
                after_state: Some(serde_json::json!(
                    payload
                        .recipe_ids
                        .iter()
                        .map(|id| serde_json::json!({
                            "recipe_id": id,
                            "source": "admin",
                        }))
                        .collect::<Vec<_>>()
                )),
            },
        )
        .await
    {
        error!("audit_log 写入失败(agent.grant_recipes): {}", e);
    }

    Ok(Json(GrantRecipesResponse {
        success: true,
        message,
        granted_count: granted,
    }))
}
