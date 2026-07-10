// ============================================================================
// Emergence Dashboard Handler（因果涌现检测仪表盘）
// ============================================================================
//
// 接口契约：
// GET /api/dashboard/emergence?window=240&start=&end=&health=1
//   → 两阶段涌现检测结果（causal_emergence + co_occurrence 事件链）
//   → 可选 health=1 附带 MVP §6.1 健康度
//
// 数据来源：agent_action_logs / agent_states / agents（只读查询）
// 架构定位：观察者职责，只读，不侵入 tick 热路径。
// ============================================================================

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::emergence::{self, DetectionResult};
use crate::state::AppState;

/// 查询参数
#[derive(Deserialize)]
pub struct EmergenceQuery {
    /// 回溯窗口大小（tick 数），默认 240（MVP 观测窗口）
    pub window: Option<i64>,
    /// 精确起始 tick_id（优先级高于 window）
    pub start: Option<i64>,
    /// 精确结束 tick_id
    pub end: Option<i64>,
    /// 是否附带 MVP 健康度（默认 false）
    pub health: Option<bool>,
}

/// GET /api/dashboard/emergence
///
/// 返回因果涌现检测结果。默认取最近 240 tick 窗口。
pub async fn get_emergence(
    State(state): State<Arc<AppState>>,
    Query(q): Query<EmergenceQuery>,
) -> Result<Json<DetectionResult>, (StatusCode, String)> {
    // 加载配置（fail-fast）
    let config = emergence::load_emergence_config(&state.config_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // 解析窗口（start/end 优先；否则 window 回溯；window 缺省 240）
    let tick_end = match q.end {
        Some(e) => e,
        None => {
            emergence::loader::current_max_tick(&state.db_pool)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        }
    };
    let tick_start = match q.start {
        Some(s) => s,
        None => (tick_end - q.window.unwrap_or(240)).max(0),
    };

    let include_health = q.health.unwrap_or(false);

    let result = emergence::detect_window(
        &state.db_pool,
        &config,
        tick_start,
        tick_end,
        include_health,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(result))
}
