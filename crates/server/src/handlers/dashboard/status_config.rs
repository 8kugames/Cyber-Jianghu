use axum::{
    Json,
    extract::{Path, State},
};
use serde::Serialize;
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

use crate::state::AppState;

// ============================================================================
// 状态配置 API（数据驱动）
// ============================================================================

/// 状态配置项
#[derive(Serialize)]
pub struct StatusConfig {
    pub key: String,
    pub display_name: String,
    pub description: String,
    pub color: String,
    pub sort_order: i32,
}
/// 获取状态配置列表
pub async fn get_status_configs(State(state): State<Arc<AppState>>) -> Json<Vec<StatusConfig>> {
    let gd = state.game_data.get();
    let configs: Vec<StatusConfig> = gd
        .game_rules
        .data
        .agent_statuses
        .iter()
        .map(|(key, cfg)| StatusConfig {
            key: key.clone(),
            display_name: cfg.display_name.clone(),
            description: cfg.description.clone(),
            color: cfg.color.clone(),
            sort_order: cfg.sort_order,
        })
        .collect();
    Json(configs)
}
#[derive(Serialize)]
pub struct AgentDetail {
    pub id: Uuid,
    pub name: String,
    pub system_prompt: String,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_active: Option<chrono::DateTime<chrono::Utc>>,
    pub location: String,
    pub hp: i32,
    pub max_hp: i32,
    pub hunger: i32,
    pub max_hunger: i32,
    pub thirst: i32,
    pub max_thirst: i32,
    pub stamina: i32,
    pub max_stamina: i32,
    pub is_alive: bool,
    pub inventory: Vec<AgentInventoryItem>,
    pub attributes: std::collections::HashMap<String, i32>,
    /// 当前年龄（游戏年），NULL = 不朽
    pub age: Option<i64>,
    pub max_age: Option<i64>,
    pub biography: Option<String>,
    pub roles: Vec<String>,
}

#[derive(Serialize)]
pub struct AgentInventoryItem {
    pub item_id: String,
    pub name: String,
    pub count: i32,
    pub is_equipped: bool,
}

pub async fn get_agent_details(
    State(state): State<Arc<AppState>>,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<AgentDetail>, axum::http::StatusCode> {
    // 1. Get basic info
    let agent_row = sqlx::query("SELECT * FROM agents WHERE agent_id = $1")
        .bind(agent_id)
        .fetch_optional(&state.db_pool)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    let agent_row = match agent_row {
        Some(row) => row,
        None => return Err(axum::http::StatusCode::NOT_FOUND),
    };

    // 2. Get latest state
    let state_row =
        sqlx::query("SELECT * FROM agent_states WHERE agent_id = $1 ORDER BY tick_id DESC LIMIT 1")
            .bind(agent_id)
            .fetch_optional(&state.db_pool)
            .await
            .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    // 3. Get inventory
    let inventory_rows = sqlx::query(
        "SELECT ai.item_id, i.name, ai.quantity, ai.is_equipped
         FROM agent_inventory ai
         JOIN items i ON ai.item_id = i.item_id
         WHERE ai.agent_id = $1",
    )
    .bind(agent_id)
    .fetch_all(&state.db_pool)
    .await
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    let inventory = inventory_rows
        .into_iter()
        .map(|row| AgentInventoryItem {
            item_id: row.get("item_id"),
            name: row.get("name"),
            count: row.get("quantity"),
            is_equipped: row.get("is_equipped"),
        })
        .collect();

    let roles = sqlx::query_scalar::<_, String>(
        "SELECT role_key FROM agent_assigned_roles WHERE agent_id = $1 ORDER BY role_key",
    )
    .bind(agent_id)
    .fetch_all(&state.db_pool)
    .await
    .unwrap_or_default();

    let (
        location,
        hp,
        max_hp,
        hunger,
        max_hunger,
        thirst,
        max_thirst,
        stamina,
        max_stamina,
        is_alive,
        mut attributes_map,
    ) = if let Some(ref row) = state_row {
        // 从 JSONB attributes 列提取属性值
        let attrs: serde_json::Value = row.get::<serde_json::Value, _>("attributes");

        let mut attributes_map = std::collections::HashMap::new();
        if let Some(obj) = attrs.as_object() {
            for (k, v) in obj {
                if let Some(val) = v.as_i64() {
                    attributes_map.insert(k.clone(), val as i32);
                }
            }
        }

        // 获取配置计算动态最大值
        let config = crate::game_data::registry::StateRegistry::get_attributes_config();
        let get_max = |name: &str| -> i32 {
            if let Some(cfg) = &config
                && let Some(attr_def) = cfg.data.status.attributes.get(name)
            {
                return crate::game_data::types::StatusComponent::evaluate_max_value(
                    &attr_def.max_value_formula,
                    100.0,
                    &attributes_map,
                ) as i32;
            }
            100
        };

        (
            row.get::<String, _>("node_id"),
            attrs.get("hp").and_then(|v| v.as_i64()).unwrap_or(100) as i32,
            attrs
                .get("hp_max")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32)
                .unwrap_or_else(|| get_max("hp")),
            attrs.get("hunger").and_then(|v| v.as_i64()).unwrap_or(100) as i32,
            attrs
                .get("hunger_max")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32)
                .unwrap_or_else(|| get_max("hunger")),
            attrs.get("thirst").and_then(|v| v.as_i64()).unwrap_or(100) as i32,
            attrs
                .get("thirst_max")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32)
                .unwrap_or_else(|| get_max("thirst")),
            attrs.get("stamina").and_then(|v| v.as_i64()).unwrap_or(100) as i32,
            attrs
                .get("stamina_max")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32)
                .unwrap_or_else(|| get_max("stamina")),
            row.get::<bool, _>("is_alive"),
            attributes_map,
        )
    } else {
        (
            "unknown".to_string(),
            100,
            100,
            100,
            100,
            100,
            100,
            100,
            100,
            true,
            std::collections::HashMap::new(),
        )
    };

    // Calculate derived attributes
    if let Some(cfg) = crate::game_data::registry::StateRegistry::get_attributes_config() {
        let mut base_attrs = std::collections::HashMap::new();
        for (k, v) in &cfg.data.derived.attributes {
            base_attrs.insert(
                k.clone(),
                cyber_jianghu_protocol::AttributeMetadata {
                    name: v.name.clone(),
                    display_name: v.display_name.clone(),
                    description: v.description.clone(),
                    formula: v.formula.clone(),
                    affects: vec![],
                    attr_type: cyber_jianghu_protocol::AttributeType::Derived,
                    birth_range: None,
                    default_value: None,
                    min_value: None,
                    max_value_formula: None,
                    decay_per_tick: None,
                    death_condition: None,
                    initial_value: None,
                    growth_rate: None,
                    recovery_formula: None,
                    primary_attribute_deps: vec![],
                },
            );
        }
        let derived_component =
            crate::game_data::types::components::DerivedAttributeComponent::from_config(
                &base_attrs,
            );
        let formula_engine = crate::game_data::formula_engine::FormulaEngine::new();

        let mut context_i64 = std::collections::HashMap::new();
        for (k, v) in &attributes_map {
            context_i64.insert(k.clone(), *v as f64);
        }

        for name in cfg.data.derived.attributes.keys() {
            if let Ok(val) = derived_component.calculate(name, &formula_engine, &context_i64) {
                attributes_map.insert(name.clone(), val as i32);
            }
        }
    }

    // 计算年龄与寿元
    let age = if let Some(birth_tick) = agent_row.get::<Option<i64>, _>("birth_tick") {
        let current_tick = state_row
            .as_ref()
            .map(|r| r.get::<i64, _>("tick_id"))
            .unwrap_or(0);
        if birth_tick > 0 && birth_tick < current_tick {
            Some(crate::tick::decay::compute_age_years(
                birth_tick,
                current_tick,
            ))
        } else {
            Some(0)
        }
    } else {
        None
    };
    let max_age = state
        .game_data
        .get_lifespan_config()
        .map(|(m, _, _)| m as i64);

    Ok(Json(AgentDetail {
        id: agent_row.get("agent_id"),
        name: agent_row.get("name"),
        system_prompt: agent_row.get("system_prompt"),
        created_at: agent_row.get("created_at"),
        last_active: agent_row.get("last_tick_online"),
        location,
        hp,
        max_hp,
        hunger,
        max_hunger,
        thirst,
        max_thirst,
        stamina,
        max_stamina,
        is_alive,
        inventory,
        attributes: attributes_map,
        age,
        max_age,
        biography: agent_row.get("biography"),
        roles,
    }))
}

// ============================================================================
// Maintenance API
// ============================================================================
