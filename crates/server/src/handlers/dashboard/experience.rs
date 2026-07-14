use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::state::AppState;

/// 经历日志条目
#[derive(Debug, serde::Serialize)]
pub struct ExperienceEntry {
    pub tick_id: i64,
    /// 动作原始类型（如 idle, speak）
    pub action_type: String,
    /// 动作中文描述（如 "静修"、"交谈"）
    pub action_type_display: Option<String>,
    pub action_data: serde_json::Value,
    /// 执行结果（success/failed）
    pub result: Option<String>,
    /// 执行结果详细描述
    pub result_message: Option<String>,
    /// ActorSoul 思考日志
    pub thought_log: Option<String>,
    /// ReflectorSoul 审查理由
    pub reflector_thought: Option<String>,
    /// 叙事化经历描述
    pub narrative: Option<String>,
    /// 三魂循环元数据
    #[serde(skip_serializing_if = "Option::is_none")]
    pub soul_cycle_metadata: Option<serde_json::Value>,
    /// 游戏日编号（从 soul_cycle_metadata.world_time 解析，无元数据时为 0）
    pub game_day: i64,
    /// 中文时间（由 `WorldTime::to_chinese()` 生成，无法解析时为 "-"），供前端直接渲染
    pub formatted_time: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// 经历日志响应
#[derive(Debug, serde::Serialize)]
pub struct ExperiencesResponse {
    pub experiences: Vec<ExperienceEntry>,
    pub total: i64,
    pub page: i32,
    pub limit: i32,
}

/// 获取 Agent 经历日志
///
/// 支持两种认证方式：
/// 1. Admin token (Bearer auth): 查看任意角色的经历日志
/// 2. Device auth (query params): 设备只能查看自己归属角色的经历日志
///
/// GET /api/dashboard/agent/{id}/experiences?page=1&limit=20&device_id=xxx&auth_token=yyy
pub async fn get_agent_experiences(
    State(state): State<Arc<AppState>>,
    Path(agent_id): Path<Uuid>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<ExperiencesResponse>, StatusCode> {
    // 设备认证：如果提供了 device_id 和 auth_token，使用设备归属校验
    if let (Some(device_id_str), Some(auth_token)) =
        (params.get("device_id"), params.get("auth_token"))
        && let Ok(device_id) = Uuid::parse_str(device_id_str)
    {
        match crate::db::verify_device_token(&state.db_pool, device_id, auth_token).await {
            Ok(true) => {
                // 验证通过，检查设备是否归属该 agent
                let owner_device_id: Option<Uuid> =
                    sqlx::query_scalar("SELECT device_id FROM agents WHERE agent_id = $1")
                        .bind(agent_id)
                        .fetch_optional(&state.db_pool)
                        .await
                        .unwrap_or(None);

                if owner_device_id != Some(device_id) {
                    tracing::warn!(
                        "Device {} attempted to access agent {} experiences without ownership",
                        device_id,
                        agent_id
                    );
                    return Err(StatusCode::FORBIDDEN);
                }
            }
            Ok(false) => return Err(StatusCode::UNAUTHORIZED),
            Err(e) => {
                tracing::warn!("Device token verify error: {}", e);
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
    }

    let page: i32 = params.get("page").and_then(|s| s.parse().ok()).unwrap_or(1);
    let limit: i32 = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    let offset = (page - 1) * limit;

    // 获取经历日志总 tick 数（按 tick_id 分组计数）
    let total: i64 =
        sqlx::query_scalar("SELECT COUNT(DISTINCT tick_id) FROM agent_action_logs WHERE agent_id = $1")
            .bind(agent_id)
            .fetch_one(&state.db_pool)
            .await
            .unwrap_or(0);

    // 先获取分页的 tick_id 列表，再批量拉取全部 pipe_seq 行
    let tick_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT tick_id
         FROM agent_action_logs
         WHERE agent_id = $1
         ORDER BY tick_id DESC
         LIMIT $2 OFFSET $3",
    )
    .bind(agent_id)
    .bind(limit as i64)
    .bind(offset as i64)
    .fetch_all(&state.db_pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch experience tick_ids: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if tick_ids.is_empty() {
        return Ok(Json(ExperiencesResponse {
            experiences: Vec::new(),
            total,
            page,
            limit,
        }));
    }

    // 构建 IN 子句参数（sqlx 不支持变长 IN，用 = ANY 替代）
    let rows = sqlx::query(
        "SELECT tick_id, action_type, action_type_display, action_data, result, result_message,
                thought_log, reflector_thought, narrative, soul_cycle_metadata, pipe_seq, created_at
         FROM agent_action_logs
         WHERE agent_id = $1 AND tick_id = ANY($2)
         ORDER BY tick_id DESC, pipe_seq ASC",
    )
    .bind(agent_id)
    .bind(&tick_ids)
    .fetch_all(&state.db_pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch experiences: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // 按 tick_id 分组，合并多 pipe_seq 行为 execution_results
    use std::collections::HashMap;
    let mut grouped: HashMap<i64, Vec<sqlx::postgres::PgRow>> = HashMap::new();
    for row in rows {
        let tid: i64 = row.get("tick_id");
        grouped.entry(tid).or_default().push(row);
    }

    let experiences: Vec<ExperienceEntry> = grouped
        .into_iter()
        .map(|(_tid, group)| {
            // 按 pipe_seq 升序排列（已由 SQL 保证）
            // pipe_seq=0 行为主行，含完整三魂元数据
            let primary = group.first().expect("group must have at least one row");
            let metadata: Option<serde_json::Value> = primary.get("soul_cycle_metadata");
            let world_time_json: Option<String> = metadata
                .as_ref()
                .and_then(|m| m.get("world_time"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // 构建 execution_results: key=pipe_seq → {success, error, state_change_summary}
            let execution_results: Option<serde_json::Value> = {
                let mut map = serde_json::Map::new();
                for row in &group {
                    let pipe_seq: i32 = row.get("pipe_seq");
                    let result: Option<String> = row.get("result");
                    let result_msg: Option<String> = row.get("result_message");
                    let is_success = result.as_deref() == Some("success");
                    map.insert(
                        pipe_seq.to_string(),
                        serde_json::json!({
                            "success": is_success,
                            "error": if is_success { serde_json::Value::Null } else { serde_json::Value::String(result_msg.clone().unwrap_or_default()) },
                            "state_change_summary": if is_success { serde_json::Value::String(result_msg.clone().unwrap_or_default()) } else { serde_json::Value::Null },
                        }),
                    );
                }
                Some(serde_json::Value::Object(map))
            };

            // 将 execution_results 注入 soul_cycle_metadata
            let enriched_metadata = metadata.map(|mut m| {
                if let serde_json::Value::Object(ref mut obj) = m {
                    obj.insert("execution_results".to_string(), execution_results.clone().unwrap_or(serde_json::Value::Null));
                }
                m
            });

            ExperienceEntry {
                tick_id: primary.get("tick_id"),
                action_type: primary.get("action_type"),
                action_type_display: primary.get("action_type_display"),
                action_data: primary
                    .get::<Option<serde_json::Value>, _>("action_data")
                    .unwrap_or(serde_json::Value::Null),
                result: primary.get("result"),
                result_message: primary.get("result_message"),
                thought_log: primary.get("thought_log"),
                reflector_thought: primary.get("reflector_thought"),
                narrative: primary.get("narrative"),
                soul_cycle_metadata: enriched_metadata,
                game_day: crate::time_utils::world_time_json_to_game_day(
                    world_time_json.as_deref(),
                ),
                formatted_time: crate::time_utils::world_time_json_to_chinese(
                    world_time_json.as_deref(),
                ),
                created_at: primary.get("created_at"),
            }
        })
        .collect();

    Ok(Json(ExperiencesResponse {
        experiences,
        total,
        page,
        limit,
    }))
}

/// GET /api/dashboard/actions-map - 返回 action_type -> 中文名映射
///
/// 无需认证（action 映射不是敏感数据，供前端渲染使用）
pub async fn get_actions_map() -> Json<std::collections::HashMap<String, String>> {
    let map: std::collections::HashMap<String, String> =
        crate::game_data::ActionRegistry::build_available_actions()
            .into_iter()
            .map(|a| (a.action, a.name))
            .collect();
    Json(map)
}

// ============================================================================
// Experience Stream API (经历日志流水)
// ============================================================================

/// 经历日志流水查询参数
#[derive(Debug, Deserialize)]
pub struct ExperienceStreamQuery {
    pub page: Option<i32>,
    pub limit: Option<i32>,
    pub agent_id: Option<Uuid>,
    pub location: Option<String>,
    pub action_type: Option<String>,
    pub from_tick: Option<i64>,
    pub to_tick: Option<i64>,
    /// 结果过滤: "success" | "failed" | 空=全部
    pub result: Option<String>,
}

/// 经历日志流水条目
#[derive(Debug, Serialize)]
pub struct StreamEntry {
    pub tick_id: i64,
    pub agent_id: Uuid,
    pub device_id: Option<Uuid>,
    pub agent_name: String,
    pub location: Option<String>,
    pub action_type: String,
    pub action_type_display: Option<String>,
    pub action_data: serde_json::Value,
    pub result: Option<String>,
    pub result_message: Option<String>,
    pub thought_log: Option<String>,
    pub reflector_thought: Option<String>,
    pub narrative: Option<String>,
    pub soul_cycle_metadata: Option<serde_json::Value>,
    /// 游戏日编号（0 表示无元数据）
    pub game_day: i64,
    /// 中文时间（由 `WorldTime::to_chinese()` 生成，无法解析时为 "-"），供前端直接渲染
    pub formatted_time: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// 经历日志流水响应
#[derive(Debug, Serialize)]
pub struct ExperienceStreamResponse {
    pub entries: Vec<StreamEntry>,
    pub total: i64,
    pub page: i32,
    pub limit: i32,
}

/// GET /api/dashboard/experiences
///
/// 返回 agent 动作日志（全局视图），用于经历日志流水。
/// 默认只返回成功记录，传 result=all 查看全部。
pub async fn get_experiences(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExperienceStreamQuery>,
) -> Result<Json<ExperienceStreamResponse>, StatusCode> {
    let page = params.page.unwrap_or(1).max(1);
    let limit = params.limit.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * limit;

    // 构建过滤条件
    let agent_id_filter = params.agent_id;
    let location_filter = params.location;
    let action_type_filter = params.action_type;
    let from_tick_filter = params.from_tick;
    let to_tick_filter = params.to_tick;
    // result 过滤: None/空 → 只看成功, "failed" → 只看失败, "all" → 全部
    let result_filter = params.result.as_deref().unwrap_or("success");

    // 查询总数
    let total: i64 = sqlx::query_scalar(
        r#"
        WITH action_with_location AS (
            SELECT a.tick_id, a.agent_id,
                   loc.node_id as location
            FROM agent_action_logs a
            LEFT JOIN LATERAL (
                SELECT st2.node_id
                FROM agent_states st2
                WHERE st2.agent_id = a.agent_id AND st2.tick_id <= a.tick_id
                ORDER BY st2.tick_id DESC
                LIMIT 1
            ) loc ON true
            WHERE ($6::text = 'all' OR a.result = $6)
              AND ($1::uuid IS NULL OR a.agent_id = $1)
              AND ($3::text IS NULL OR a.action_type = $3)
              AND ($4::bigint IS NULL OR a.tick_id >= $4)
              AND ($5::bigint IS NULL OR a.tick_id <= $5)
        )
        SELECT COUNT(*)
        FROM action_with_location
        WHERE ($2::text IS NULL OR location = $2)
        "#,
    )
    .bind(agent_id_filter)
    .bind(&location_filter)
    .bind(&action_type_filter)
    .bind(from_tick_filter)
    .bind(to_tick_filter)
    .bind(result_filter)
    .fetch_one(&state.db_pool)
    .await
    .unwrap_or(0);

    // 查询条目：使用 LATERAL JOIN 获取动作发生时的位置
    let rows = sqlx::query(
        r#"
        SELECT a.tick_id, a.agent_id, ag.device_id, ag.name as agent_name, loc.node_id as location,
               a.action_type, a.action_type_display, a.action_data,
               a.result, a.result_message, a.thought_log, a.reflector_thought,
               a.narrative, a.soul_cycle_metadata, a.created_at
        FROM agent_action_logs a
        JOIN agents ag ON a.agent_id = ag.agent_id
        LEFT JOIN LATERAL (
            SELECT st2.node_id
            FROM agent_states st2
            WHERE st2.agent_id = a.agent_id AND st2.tick_id <= a.tick_id
            ORDER BY st2.tick_id DESC
            LIMIT 1
        ) loc ON true
        WHERE ($6::text = 'all' OR a.result = $6)
          AND ($1::uuid IS NULL OR a.agent_id = $1)
          AND ($2::text IS NULL OR loc.node_id = $2)
          AND ($3::text IS NULL OR a.action_type = $3)
          AND ($4::bigint IS NULL OR a.tick_id >= $4)
          AND ($5::bigint IS NULL OR a.tick_id <= $5)
        ORDER BY a.tick_id DESC
        LIMIT $7 OFFSET $8
        "#,
    )
    .bind(agent_id_filter)
    .bind(&location_filter)
    .bind(&action_type_filter)
    .bind(from_tick_filter)
    .bind(to_tick_filter)
    .bind(result_filter)
    .bind(limit as i64)
    .bind(offset as i64)
    .fetch_all(&state.db_pool)
    .await
    .map_err(|e| {
        tracing::error!("获取经历日志流水失败: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let entries: Vec<StreamEntry> = rows
        .into_iter()
        .map(|row| {
            let metadata: Option<serde_json::Value> = row.get("soul_cycle_metadata");
            let world_time_json: Option<String> = metadata
                .as_ref()
                .and_then(|m| m.get("world_time"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            StreamEntry {
                tick_id: row.get("tick_id"),
                agent_id: row.get("agent_id"),
                device_id: row.get("device_id"),
                agent_name: row.get("agent_name"),
                location: row.get("location"),
                action_type: row.get("action_type"),
                action_type_display: row.get("action_type_display"),
                action_data: row
                    .get::<Option<serde_json::Value>, _>("action_data")
                    .unwrap_or(serde_json::Value::Null),
                result: row.get("result"),
                result_message: row.get("result_message"),
                thought_log: row.get("thought_log"),
                reflector_thought: row.get("reflector_thought"),
                narrative: row.get("narrative"),
                soul_cycle_metadata: metadata,
                game_day: crate::time_utils::world_time_json_to_game_day(
                    world_time_json.as_deref(),
                ),
                formatted_time: crate::time_utils::world_time_json_to_chinese(
                    world_time_json.as_deref(),
                ),
                created_at: row.get("created_at"),
            }
        })
        .collect();

    Ok(Json(ExperienceStreamResponse {
        entries,
        total,
        page,
        limit,
    }))
}

// ============================================================================
// Items API (物品列表，供 Admin 面板 grant-items UI 使用)
// ============================================================================

/// 物品摘要（用于下拉选择器）
#[derive(Debug, Serialize)]
pub struct ItemSummary {
    pub item_id: String,
    pub name: String,
    pub item_type: String,
    pub description: String,
}

/// 获取所有已配置物品列表
///
/// GET /api/dashboard/items
pub async fn get_items() -> Json<Vec<ItemSummary>> {
    let items = crate::game_data::registry::ItemRegistry::all_item_ids()
        .iter()
        .filter_map(|id| crate::game_data::registry::ItemRegistry::get(id))
        .map(|entry| ItemSummary {
            item_id: entry.item_id,
            name: entry.name,
            item_type: entry.item_type,
            description: entry.description,
        })
        .collect();
    Json(items)
}

// ============================================================================
// Display Map API（展示名映射，供经历日志前端翻译 agent_id / item_id）
// ============================================================================

/// 展示名映射响应
///
/// - `items`：item_id → 物品名（来自 items.yaml 权威配置源）
/// - `agents`：agent_id → 角色名（来自 agents 表，含全部状态：在线/离线/死亡）
#[derive(Debug, Serialize)]
pub struct DisplayMapResponse {
    pub items: HashMap<String, String>,
    pub agents: HashMap<String, String>,
}

/// 获取展示名映射
///
/// GET /api/dashboard/display-map
///
/// 前端经历日志在渲染前拉取本端点，用于将 action_data 中的 target_agent_id
/// 翻译为角色名。agents 映射查全表（无状态过滤、无 LIMIT），覆盖历史日志中
/// 已死亡/离线的目标角色——这是根治"角色 ID 未翻译"的单一数据源。
pub async fn get_display_map(
    State(state): State<Arc<AppState>>,
) -> Result<Json<DisplayMapResponse>, StatusCode> {
    // items：从物品配置注册表（items.yaml）生成，单一权威源、零硬编码
    let items: HashMap<String, String> =
        crate::game_data::registry::ItemRegistry::all_item_ids()
            .iter()
            .filter_map(|id| {
                crate::game_data::registry::ItemRegistry::get(id)
                    .map(|entry| (entry.item_id, entry.name))
            })
            .collect();

    // agents：一条轻量 SQL，全状态、无 JOIN
    let rows = sqlx::query("SELECT agent_id, name FROM agents")
        .fetch_all(&state.db_pool)
        .await
        .map_err(|e| {
            tracing::error!("display-map 查询 agents 失败: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let agents: HashMap<String, String> = rows
        .iter()
        .map(|r| {
            let aid: Uuid = r.get("agent_id");
            let name: String = r.get("name");
            (aid.to_string(), name)
        })
        .collect();

    Ok(Json(DisplayMapResponse { items, agents }))
}

/// 天魂层展示名映射（数据驱动，从 souls.yaml layer_display 读取）
///
/// GET /api/dashboard/layer-display
pub async fn get_layer_display(
    State(state): State<Arc<AppState>>,
) -> Result<Json<std::collections::HashMap<String, String>>, StatusCode> {
    let yaml_path = state.config_dir.join("souls.yaml");
    let content = std::fs::read_to_string(&yaml_path).map_err(|e| {
        tracing::error!("读取 souls.yaml 失败: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let parsed: serde_json::Value = serde_yaml::from_str(&content).map_err(|e| {
        tracing::error!("解析 souls.yaml 失败: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let map = parsed
        .get("data")
        .and_then(|d| d.get("tianhun"))
        .and_then(|t| t.get("layer_display"))
        .map(|v| {
            serde_json::from_value::<std::collections::HashMap<String, String>>(v.clone())
                .unwrap_or_else(|e| {
                    tracing::warn!("souls.yaml layer_display 字段解析失败: {}", e);
                    std::collections::HashMap::new()
                })
        })
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| {
            // 向后兼容：若无配置，返回默认映射
            let mut m = std::collections::HashMap::new();
            m.insert("layer1".to_string(), "动作审查".to_string());
            m.insert("layer2".to_string(), "规则校验".to_string());
            m.insert("layer3".to_string(), "意图审查".to_string());
            m
        });

    Ok(Json(map))
}
