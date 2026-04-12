use crate::game_data::registry_or_error;
use crate::game_data::types::unified_config::{SeasonData, TimeData};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeDisplay {
    pub tick_id: i64,
    pub season: Option<SeasonData>,
    pub hour: i32,
    pub day: i32,
    pub is_daytime: bool,
}

/// 时间注册表
///
/// 提供对时间与季节配置的安全访问
pub struct TimeRegistry;

impl TimeRegistry {
    /// 获取完整时间配置
    pub fn get_config() -> Option<TimeData> {
        let registry = registry_or_error().ok()?;
        Some(registry.get().time.data.clone())
    }

    /// 根据 tick 获取当前季节
    pub fn get_current_season(current_tick: i64) -> Option<SeasonData> {
        let config = Self::get_config()?;
        let registry = registry_or_error().ok()?;
        let tick_config = &registry.get().game_rules.data.agent_state.tick;

        let ticks_per_hour = config.ticks_per_hour as i64;
        let hours_per_day = config.hours_per_day as i64;
        let days_per_season = config.days_per_season as i64;
        let real_seconds_per_tick = tick_config.real_seconds_per_tick as i64;

        // tick_id 是秒级秒数，转换为游戏小时
        // game_hours = tick_id / (real_seconds_per_tick * ticks_per_hour)
        let real_seconds_per_game_hour = real_seconds_per_tick * ticks_per_hour;
        let game_hours = if real_seconds_per_game_hour > 0 {
            current_tick / real_seconds_per_game_hour
        } else {
            current_tick / ticks_per_hour
        };
        let game_days = game_hours / hours_per_day;
        let game_seasons = game_days / days_per_season;

        // 季节循环索引
        let season_count = config.seasons.len() as i64;
        if season_count == 0 {
            return None;
        }

        let season_index = (game_seasons % season_count) as usize;

        config.seasons.get(season_index).cloned()
    }

    /// 获取格式化的时间显示，用于广播
    pub fn get_time_display(current_tick: i64) -> Option<TimeDisplay> {
        let config = Self::get_config()?;
        let registry = registry_or_error().ok()?;
        let tick_config = &registry.get().game_rules.data.agent_state.tick;

        let ticks_per_hour = config.ticks_per_hour as i64;
        let hours_per_day = config.hours_per_day as i64;
        let real_seconds_per_tick = tick_config.real_seconds_per_tick as i64;

        // tick_id 是秒级秒数，转换为游戏小时
        // game_hours = tick_id / (real_seconds_per_tick * ticks_per_hour)
        let real_seconds_per_game_hour = real_seconds_per_tick * ticks_per_hour;
        let game_hours = if real_seconds_per_game_hour > 0 {
            current_tick / real_seconds_per_game_hour
        } else {
            current_tick / ticks_per_hour
        };
        let hour_of_day = game_hours % hours_per_day;
        let day_of_season = (game_hours / hours_per_day) % config.days_per_season as i64;

        // 假设 6:00 到 18:00 为白天
        let is_daytime = (6..18).contains(&hour_of_day);

        let season = Self::get_current_season(current_tick);

        Some(TimeDisplay {
            tick_id: current_tick,
            season,
            hour: hour_of_day as i32,
            day: day_of_season as i32 + 1, // Day 1-based
            is_daytime,
        })
    }
}
