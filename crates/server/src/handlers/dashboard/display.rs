use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::state::AppState;

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
// Display Map API（展示名映射，供经历日志前端翻译 agent_id / item_id）
// ============================================================================

/// 展示名映射响应
///
/// - `items`：item_id → 物品名（来自 items.yaml 权威配置源）
/// - `item_uuids`：item_id → 稳定 uuid（UUID v5 派生；展示用短 uuid 取前 8 位）
/// - `recipes`：recipe_id → 配方名（来自 recipes.yaml 权威配置源）
/// - `recipe_uuids`：recipe_id → 稳定 uuid（UUID v5 派生）
/// - `agents`：agent_id → 角色名（来自 agents 表，含全部状态：在线/离线/死亡）
#[derive(Debug, Serialize)]
pub struct DisplayMapResponse {
    pub items: HashMap<String, String>,
    pub item_uuids: HashMap<String, String>,
    pub recipes: HashMap<String, String>,
    pub recipe_uuids: HashMap<String, String>,
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
    let items: HashMap<String, String> = crate::game_data::registry::ItemRegistry::all_item_ids()
        .iter()
        .filter_map(|id| {
            crate::game_data::registry::ItemRegistry::get(id)
                .map(|entry| (entry.item_id, entry.name))
        })
        .collect();

    // item uuids：与 items 同源的稳定标识，供前端拼装 名称[短uuid] 展示
    let item_uuids: HashMap<String, String> = items
        .keys()
        .map(|id| (id.clone(), crate::items::item_uuid(id).to_string()))
        .collect();

    // recipes：配方名 + 稳定 uuid（配方是制造/传授的前提，前端经历日志需要翻译 recipe_id）
    let recipes: HashMap<String, String> = crate::game_data::registry::RecipeRegistry::all()
        .into_iter()
        .collect();
    let recipe_uuids: HashMap<String, String> = recipes
        .keys()
        .map(|id| (id.clone(), crate::display::recipe_uuid(id).to_string()))
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

    Ok(Json(DisplayMapResponse {
        items,
        item_uuids,
        recipes,
        recipe_uuids,
        agents,
    }))
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
