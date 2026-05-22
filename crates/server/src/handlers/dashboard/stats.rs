use axum::{
    Json,
    extract::State,
};
use chrono::Utc;
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

use crate::state::AppState;
use cyber_jianghu_protocol::types::world::{number_to_chinese, shichen_name};

use super::types::*;

pub async fn get_dashboard_stats(State(state): State<Arc<AppState>>) -> Json<DashboardStats> {
    // 1. Total registered agents
    let total_registered_agents: i64 = sqlx::query("SELECT COUNT(*) FROM agents")
        .fetch_one(&state.db_pool)
        .await
        .map(|r| r.get(0))
        .unwrap_or(0);

    // 2. Current active agents (alive and in latest tick and online)
    let connected_agents: std::collections::HashSet<Uuid> = {
        let connections = state.connection_manager.read().await;
        connections.values().map(|c| c.agent_id).collect()
    };
    let latest_state_tick_id = crate::db::get_latest_state_tick_id(&state.db_pool)
        .await
        .unwrap_or(0);

    let alive_agents: Vec<Uuid> = sqlx::query(
        "SELECT agent_id FROM agent_states
         WHERE is_alive = true
         AND tick_id = $1",
    )
    .bind(latest_state_tick_id)
    .fetch_all(&state.db_pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| r.get(0))
    .collect();

    let current_active_agents = alive_agents
        .into_iter()
        .filter(|id| connected_agents.contains(id))
        .count() as i64;

    // 3. DAU (Active in last 24h)
    // Active means: submitted an intent (last_tick_online) OR created in the last 24h
    let dau: i64 = sqlx::query(
        "SELECT COUNT(DISTINCT agent_id) FROM agents
         WHERE last_tick_online > NOW() - INTERVAL '1 day'
         OR created_at > NOW() - INTERVAL '1 day'",
    )
    .fetch_one(&state.db_pool)
    .await
    .map(|r| r.get(0))
    .unwrap_or(0);

    // 4. 3-Day Active
    let active_3d: i64 = sqlx::query(
        "SELECT COUNT(DISTINCT agent_id) FROM agents
         WHERE last_tick_online > NOW() - INTERVAL '3 days'
         OR created_at > NOW() - INTERVAL '3 days'",
    )
    .fetch_one(&state.db_pool)
    .await
    .map(|r| r.get(0))
    .unwrap_or(0);

    // 5. 7-Day Active
    let active_7d: i64 = sqlx::query(
        "SELECT COUNT(DISTINCT agent_id) FROM agents
         WHERE last_tick_online > NOW() - INTERVAL '7 days'
         OR created_at > NOW() - INTERVAL '7 days'",
    )
    .fetch_one(&state.db_pool)
    .await
    .map(|r| r.get(0))
    .unwrap_or(0);

    // 6. MAU (30 days)
    let mau: i64 = sqlx::query(
        "SELECT COUNT(DISTINCT agent_id) FROM agents
         WHERE last_tick_online > NOW() - INTERVAL '30 days'
         OR created_at > NOW() - INTERVAL '30 days'",
    )
    .fetch_one(&state.db_pool)
    .await
    .map(|r| r.get(0))
    .unwrap_or(0);

    // 7. YAU (1 year)
    let yau: i64 = sqlx::query(
        "SELECT COUNT(DISTINCT agent_id) FROM agents
         WHERE last_tick_online > NOW() - INTERVAL '1 year'
         OR created_at > NOW() - INTERVAL '1 year'",
    )
    .fetch_one(&state.db_pool)
    .await
    .map(|r| r.get(0))
    .unwrap_or(0);

    // 8. Server uptime
    let now = Utc::now();
    let uptime = now.signed_duration_since(state.start_time);
    let server_uptime_secs = uptime.num_seconds();
    let server_running_days = uptime.num_days();

    // 9. Game time - 基础数据供前端计算平滑时间
    let current_world_tick_id = crate::db::get_current_world_tick_id(&state.db_pool)
        .await
        .unwrap_or(0);

    // 从配置读取时间参数
    let config = crate::game_data::registry::TimeRegistry::get_config();
    let ticks_per_hour = config
        .as_ref()
        .map(|c| c.ticks_per_hour as f64)
        .unwrap_or(1.0);
    let hours_per_day = config
        .as_ref()
        .map(|c| c.hours_per_day as i64)
        .unwrap_or(24);
    let days_per_month = 30;
    let months_per_year = 12;
    let hours_per_month = hours_per_day * days_per_month;
    let hours_per_year = hours_per_month * months_per_year;

    // 计算整数游戏时间（前端会自行计算平滑的小数部分）
    let total_game_hours = current_world_tick_id as i64 / ticks_per_hour as i64;

    let year = 1 + (total_game_hours / hours_per_year) as i32;
    let remaining_after_year = total_game_hours % hours_per_year;
    let month = 1 + (remaining_after_year / hours_per_month) as i32;
    let remaining_after_month = remaining_after_year % hours_per_month;
    let day = 1 + (remaining_after_month / hours_per_day) as i32;
    let hour = (remaining_after_month % hours_per_day) as i32;

    // 获取季节信息
    let season =
        crate::game_data::registry::TimeRegistry::get_current_season(current_world_tick_id)
            .map(|s| s.name)
            .unwrap_or_else(|| "未知".to_string());

    let month_text = match month {
        1 => "元月",
        2 => "二月",
        3 => "三月",
        4 => "四月",
        5 => "五月",
        6 => "六月",
        7 => "七月",
        8 => "八月",
        9 => "九月",
        10 => "十月",
        11 => "十一月",
        12 => "腊月",
        _ => unreachable!(),
    };
    let text = format!(
        "天道历{}年{}{}日{}",
        number_to_chinese(year),
        month_text,
        number_to_chinese(day),
        shichen_name(hour)
    );

    let game_time = WorldTime {
        year,
        month,
        day,
        hour,
        minute: 0, // 前端会自行计算平滑值
        second: 0, // 前端会自行计算平滑值
        season,
        text,
    };

    let game_flow_total_hours = total_game_hours;

    // 10. World overview
    // Try to load world building rules
    let world_overview = crate::websocket::types::load_world_building_rules()
        .map(|rules| rules.narrative_rules)
        .unwrap_or_else(|| "世界概览暂不可用".to_string());

    // Get tick duration from game_data
    let tick_duration_secs = {
        let gd = state.game_data.get();
        gd.game_rules.data.agent_state.tick.real_seconds_per_tick as u64
    };

    // 11. Offline distribution
    let mut less_than_1h = 0;
    let mut one_to_24h = 0;
    let mut one_to_7d = 0;
    let mut more_than_7d = 0;

    let offline_rows = sqlx::query(
        "SELECT EXTRACT(EPOCH FROM (NOW() - last_tick_online))::FLOAT8 as offline_secs
         FROM agents WHERE last_tick_online IS NOT NULL",
    )
    .fetch_all(&state.db_pool)
    .await
    .unwrap_or_default();

    for row in offline_rows {
        let secs: Option<f64> = row.get(0);
        if let Some(s) = secs {
            if s < 3600.0 {
                less_than_1h += 1;
            } else if s < 86400.0 {
                one_to_24h += 1;
            } else if s < 604800.0 {
                one_to_7d += 1;
            } else {
                more_than_7d += 1;
            }
        }
    }

    // 12. Death statistics (simplified implementation using recent state changes)
    let dead_agents = sqlx::query(
        "SELECT COUNT(*) FROM agent_states s1
         JOIN agent_states s2 ON s1.agent_id = s2.agent_id AND s1.tick_id = s2.tick_id - 1
         WHERE s1.is_alive = true AND s2.is_alive = false",
    )
    .fetch_one(&state.db_pool)
    .await
    .map(|r| r.get::<i64, _>(0))
    .unwrap_or(0);

    // 假设目前所有死亡都是自然死亡（饿死等），这里作简化处理。
    // 如果有异常导致的死亡，可以在后续按需分类。
    let natural_deaths_last_24h = dead_agents;
    let abnormal_deaths_last_24h = 0;

    Json(DashboardStats {
        current_active_agents,
        total_registered_agents,
        dau,
        active_3d,
        active_7d,
        mau,
        yau,
        server_uptime_secs,
        server_running_days,
        game_time,
        game_flow_total_hours,
        world_overview,
        tick_duration_secs,
        current_tick_id: current_world_tick_id,
        ticks_per_hour,
        natural_deaths_last_24h,
        abnormal_deaths_last_24h,
        offline_duration_distribution: OfflineDistribution {
            less_than_1h,
            one_to_24h,
            one_to_7d,
            more_than_7d,
        },
    })
}

// ============================================================================
// Agent API
// ============================================================================
//
