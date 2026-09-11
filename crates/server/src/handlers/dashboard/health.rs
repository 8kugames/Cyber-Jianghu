// ============================================================================
// Health Dashboard Handler（健康度看板）
// ============================================================================
//
// 接口契约：
// GET /api/dashboard/health?window=240
//   → MVP 验收指标的结构化视图
//
// 指标覆盖：
//   运行稳定性：tick 完成率、连续运行时长、崩溃数
//   意图超时率（近似，标注非 MVP 字面30秒墙钟口径）
//   生存能力：窗口末点存活数、人均补给次数
//   涌现：causal_emergence 计数（复用 emergence 模块）
//
// 每项标注 MVP 阈值 + pass/fail 状态。
// ============================================================================

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::emergence::{self, HealthMetrics};
use crate::state::AppState;

/// 查询参数
#[derive(Deserialize)]
pub struct HealthQuery {
    /// 回溯窗口大小（tick 数），默认 240（MVP 观测窗口）
    pub window: Option<i64>,
}

/// MVP 验收响应
#[derive(Serialize)]
pub struct MvpHealth {
    pub tick_start: i64,
    pub tick_end: i64,
    pub window_ticks: i64,
    /// MVP 运行稳定性
    pub stability: StabilityCheck,
    /// MVP 生存能力
    pub survival: SurvivalCheck,
    /// MVP 复杂交互（涌现）
    pub emergence: EmergenceCheck,
    /// MVP 行为多样性（决策动作分布熵，交叉生存状态判读）
    pub behavior: BehaviorCheck,
}

#[derive(Serialize)]
pub struct StabilityCheck {
    pub tick_completion_rate: f64,
    pub threshold: f64,
    pub pass: bool,
    pub ticks_total: i64,
    pub ticks_completed: i64,
    pub ticks_failed: i64,
    pub continuous_run_seconds: f64,
    pub continuous_run_hours: f64,
    pub threshold_hours: f64,
    pub timeout_rate_approx: f64,
    pub timeout_threshold: f64,
    pub timeout_pass: bool,
    /// 超时率是近似值（非 MVP 字面30秒墙钟超时，server 端无 deadline 概念）
    pub timeout_is_approximate: bool,
}

#[derive(Serialize)]
pub struct SurvivalCheck {
    pub agents_alive: i32,
    pub min_survivors: i32,
    pub pass: bool,
    pub per_agent_supply: Vec<AgentSupply>,
    pub min_supply_count: i32,
    pub supply_pass: bool,
}

#[derive(Serialize)]
pub struct AgentSupply {
    pub agent_id: String,
    pub supply_count: i32,
    pub meets_threshold: bool,
}

#[derive(Serialize)]
pub struct EmergenceCheck {
    pub causal_emergence_count: usize,
    pub threshold: usize,
    pub pass: bool,
    pub co_occurrence_count: usize,
    pub candidate_count: usize,
}

#[derive(Serialize)]
pub struct AgentBehavior {
    pub agent_id: String,
    /// 最高频动作名
    pub top_action: String,
    /// 最高频动作占比 0-1
    pub top_share: f64,
    /// 涉及动作种类数
    pub distinct_actions: i32,
    /// 决策总数
    pub total_decisions: i64,
    /// 窗口末点饱食度（交叉判读用）
    pub satiation: f64,
    /// 饱食惰性豁免：占比超限但饱食安稳
    pub exempted: bool,
}

#[derive(Serialize)]
pub struct BehaviorCheck {
    /// 最频动作占比上限
    pub max_top_share: f64,
    pub satiation_urgent_below: i32,
    pub pass: bool,
    pub per_agent_behavior: Vec<AgentBehavior>,
    pub fail_agents: Vec<String>,
}

/// GET /api/dashboard/health
///
/// 返回 MVP 全部验收指标 + pass/fail。
pub async fn get_health(
    State(state): State<Arc<AppState>>,
    Query(q): Query<HealthQuery>,
) -> Result<Json<MvpHealth>, (StatusCode, String)> {
    // 加载涌现配置（复用，含 health 阈值）
    let config = emergence::load_emergence_config(&state.config_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let window = q.window.unwrap_or(240);
    let tick_end = emergence::loader::current_max_tick(&state.db_pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let tick_start = (tick_end - window).max(0);

    // 复用 emergence 模块的检测 + 健康度（一次调用拿到全部数据）
    let result = emergence::detect_window(&state.db_pool, &config, tick_start, tick_end, true)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let h: &HealthMetrics = result.health.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "健康度数据缺失".to_string(),
        )
    })?;

    // 运行稳定性阈值
    let tick_rate_threshold = 0.99;
    let run_hours = h.continuous_run_seconds / 3600.0;
    let timeout_threshold = 0.05;

    let stability = StabilityCheck {
        tick_completion_rate: h.tick_completion_rate,
        threshold: tick_rate_threshold,
        pass: h.tick_completion_rate >= tick_rate_threshold,
        ticks_total: h.ticks_total,
        ticks_completed: h.ticks_completed,
        ticks_failed: h.ticks_failed,
        continuous_run_seconds: h.continuous_run_seconds,
        continuous_run_hours: run_hours,
        threshold_hours: 24.0,
        timeout_rate_approx: h.timeout_rate_approx,
        timeout_threshold,
        timeout_pass: h.timeout_rate_approx <= timeout_threshold,
        timeout_is_approximate: true,
    };

    // 生存能力
    let per_agent: Vec<AgentSupply> = {
        let mut v: Vec<AgentSupply> = h
            .per_agent_supply
            .iter()
            .map(|(id, cnt)| AgentSupply {
                agent_id: id.to_string(),
                supply_count: *cnt,
                meets_threshold: *cnt >= h.min_supply_required,
            })
            .collect();
        v.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        v
    };

    let survival = SurvivalCheck {
        agents_alive: h.agents_alive,
        min_survivors: h.min_survivors_required,
        pass: h.survivors_pass,
        per_agent_supply: per_agent,
        min_supply_count: h.min_supply_required,
        supply_pass: h.supply_pass,
    };

    // 涌现
    let emergence_check = EmergenceCheck {
        causal_emergence_count: result.causal_emergence_count,
        threshold: 1,
        pass: result.causal_emergence_count >= 1,
        co_occurrence_count: result.co_occurrence_count,
        candidate_count: result.candidate_count,
    };

    // 行为多样性（最频动作占比，交叉生存状态判读）
    let mut per_agent_behavior: Vec<AgentBehavior> = h
        .per_agent_behavior
        .iter()
        .map(|(id, b)| AgentBehavior {
            agent_id: id.to_string(),
            top_action: b.top_action.clone(),
            top_share: b.top_share,
            distinct_actions: b.distinct_actions as i32,
            total_decisions: b.total_decisions,
            satiation: b.satiation,
            exempted: b.exempted,
        })
        .collect();
    per_agent_behavior.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
    let behavior = BehaviorCheck {
        max_top_share: config.health.max_top_share,
        satiation_urgent_below: config.health.satiation_urgent_below,
        pass: h.behavior_pass,
        per_agent_behavior,
        fail_agents: h
            .behavior_fail_agents
            .iter()
            .map(|u| u.to_string())
            .collect(),
    };

    Ok(Json(MvpHealth {
        tick_start,
        tick_end,
        window_ticks: window,
        stability,
        survival,
        emergence: emergence_check,
        behavior,
    }))
}
