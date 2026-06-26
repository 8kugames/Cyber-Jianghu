// ============================================================================
// 生存 Reward 数据结构（天道账本）
// ============================================================================
//
// 哲学锚点：天道无为。reward 纯锚定生存因果。
// 身家不计入；死因 penalty 统一（不分死因）。
// ============================================================================

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 每日 reward（每游戏日末批量结算）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DailyReward {
    pub agent_id: Uuid,
    /// 当日末 tick
    pub tick_id: i64,
    /// 游戏日序号
    pub game_day: i64,
    /// 生存分量：该日存活即得（cfg.daily.survival_score）
    pub survival: f64,
    /// 生理分量：satiation/hydration 当日末值映射
    pub physiological: f64,
    /// 天魂审查分量（P1 阶段 server 读不到 agent 端，暂为 None）
    pub tianhun_judgment: Option<f64>,
    /// 合计
    pub total: f64,
}

/// 一生 reward（死亡时结算）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LifetimeReward {
    pub agent_id: Uuid,
    pub character_name: String,
    pub birth_tick: i64,
    pub death_tick: i64,
    /// 寿数（存活游戏日数）
    pub longevity_days: i64,
    /// 一生每日 reward 之和
    pub cumulative_reward: f64,
    /// 统一死亡 penalty（cfg.lifetime.death_penalty，不分死因）
    pub death_penalty: f64,
    /// 死因（仅记录叙事，从 attributes.death_cause 读，不参与 penalty）
    pub death_cause: String,
    /// 死因叙事文本（从 attributes.death_message 读）
    pub death_message: String,
    /// 一生总分 = cumulative_reward + death_penalty
    pub total: f64,
}

/// 周期 reward（复用 chronicle 7 游戏日周期）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PeriodReward {
    pub agent_id: Uuid,
    pub day_start: i64,
    pub day_end: i64,
    /// 周期内每日 reward 之和
    pub cumulative_daily_reward: f64,
    pub survived_period: bool,
}
