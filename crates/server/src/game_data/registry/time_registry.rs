use crate::game_data::registry_or_error;
use crate::game_data::types::unified_config::{SeasonData, TimeData};
use cyber_jianghu_protocol::CalendarConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeDisplay {
    pub tick_id: i64,
    pub season: Option<SeasonData>,
    pub hour: i32,
    pub day: i32,
    pub is_daytime: bool,
}

/// 退化时间配置（time.yaml 缺失/未初始化时使用，单一份定义，禁止各处拷贝字面量）
fn fallback_time_data() -> TimeData {
    TimeData {
        ticks_per_hour: 1,
        hours_per_day: 24,
        days_per_season: 10,
        seasons_per_year: 4,
        seasons: vec![],
    }
}

/// 退化 tick 秒数（game_rules 未初始化/非法值时使用，与 fallback_time_data 同源）
const FALLBACK_REAL_SECONDS_PER_TICK: i64 = 60;

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

    /// 获取日历配置（game_day 格式化专用）
    ///
    /// 协议层的 `game_day_to_chinese(game_day, &CalendarConfig)` 接受 CalendarConfig，
    /// 此处从 TimeData 投影出 CalendarConfig，避免协议层依赖服务端 TimeRegistry。
    pub fn get_calendar_config() -> Option<CalendarConfig> {
        let cfg = Self::get_config()?;
        Some(CalendarConfig {
            days_per_season: cfg.days_per_season.max(0) as u32,
            seasons_per_year: cfg.seasons_per_year.max(0) as u32,
        })
    }

    /// 根据 tick 获取当前季节
    pub fn get_current_season(current_tick: i64) -> Option<SeasonData> {
        let config = Self::get_config()?;
        let hours_per_day = config.hours_per_day as i64;
        let days_per_season = config.days_per_season as i64;
        if hours_per_day <= 0 || days_per_season <= 0 {
            return None;
        }

        let game_seasons = Self::game_hours(current_tick) / hours_per_day / days_per_season;

        // 季节循环索引
        let season_count = config.seasons.len() as i64;
        if season_count == 0 {
            return None;
        }

        let season_index = (game_seasons % season_count) as usize;

        config.seasons.get(season_index).cloned()
    }

    /// 根据 tick 获取当前天气 key（如 "sunny", "cloudy"）
    ///
    /// 确定性选择：同一 tick 内所有 agent 看到相同天气。
    /// 按 tick 推算游戏天数，对天气池取模选择。
    pub fn get_weather_key(current_tick: i64) -> String {
        let season = Self::get_current_season(current_tick);
        let pool = season
            .as_ref()
            .map(|s| s.weather_pool.as_slice())
            .unwrap_or(&[]);

        if pool.is_empty() {
            return "sunny".to_string();
        }

        // 确定性选择：用游戏天数对 pool 取模
        let game_day = {
            let hours_per_day = Self::get_config()
                .unwrap_or_else(fallback_time_data)
                .hours_per_day as i64;
            if hours_per_day <= 0 {
                0
            } else {
                (Self::game_hours(current_tick) / hours_per_day) as usize
            }
        };

        pool[game_day % pool.len()].clone()
    }

    /// tick（秒级时间戳）→ 游戏小时数
    ///
    /// 全仓唯一 tick→游戏时换算真源（时代显隐/季节/天气/寿龄/chronicle 共用），
    /// 禁止业务代码内联复制此公式。配置缺失按 fallback_time_data 退化；
    /// 参数非法（除零风险）时告警并返回 0。
    pub fn game_hours(current_tick: i64) -> i64 {
        let config = Self::get_config().unwrap_or_else(fallback_time_data);
        let ticks_per_hour = config.ticks_per_hour as i64;
        let raw_rspt = registry_or_error()
            .ok()
            .map(|r| {
                r.get()
                    .game_rules
                    .data
                    .agent_state
                    .tick
                    .real_seconds_per_tick as i64
            })
            .unwrap_or(0);
        if raw_rspt <= 0 {
            tracing::warn!(
                "game_rules real_seconds_per_tick={} 非法/不可用，退化为 FALLBACK_REAL_SECONDS_PER_TICK={}",
                raw_rspt,
                FALLBACK_REAL_SECONDS_PER_TICK
            );
        }
        let real_seconds_per_tick = if raw_rspt > 0 {
            raw_rspt
        } else {
            FALLBACK_REAL_SECONDS_PER_TICK
        };

        // tick_id 是秒级秒数：game_hours = tick / (real_seconds_per_tick * ticks_per_hour)
        let real_seconds_per_game_hour = real_seconds_per_tick * ticks_per_hour;
        if real_seconds_per_game_hour > 0 {
            current_tick / real_seconds_per_game_hour
        } else if ticks_per_hour > 0 {
            current_tick / ticks_per_hour
        } else {
            tracing::warn!(
                "time.yaml ticks_per_hour={} 非法，tick→游戏时换算退化为 0",
                ticks_per_hour
            );
            0
        }
    }

    /// 根据 tick 获取当前游戏日（1-based，单调递增）
    ///
    /// 时代显隐（locations time_variants）的统一时间基准：
    /// executor 移动校验与 broadcaster 邻接过滤都经由本函数取当前 game_day，
    /// 保证两端在同一 tick 内看到同一时代。配置缺失/非法时退化为第 1 日。
    pub fn game_day(current_tick: i64) -> i64 {
        Self::try_game_day(current_tick).unwrap_or_else(|| {
            tracing::warn!("time.yaml 时间配置不可用或非法，game_day 退化为 1");
            1
        })
    }

    /// game_day 的 fail-fast 变体：时间配置缺失/非法时返回 None
    ///
    /// 供 chronicle 等需要对配置缺失显式失败（而非接受退化值）的调用方使用。
    pub fn try_game_day(current_tick: i64) -> Option<i64> {
        let config = Self::get_config()?;
        let hours_per_day = config.hours_per_day as i64;
        if hours_per_day <= 0 {
            tracing::warn!(
                "time.yaml hours_per_day={} 非法，游戏日无法计算",
                hours_per_day
            );
            return None;
        }
        // rspt 非法同样 fail-fast：退化解会让 chronicle 周期分区以错误游戏日入库
        // （牵动 chronicle period 唯一性），不接受静默退化值。
        let rspt_valid = registry_or_error()
            .ok()
            .map(|r| {
                r.get()
                    .game_rules
                    .data
                    .agent_state
                    .tick
                    .real_seconds_per_tick
                    > 0
            })
            .unwrap_or(false);
        if !rspt_valid {
            tracing::warn!("game_rules real_seconds_per_tick 非法/不可用，游戏日拒绝计算");
            return None;
        }
        Some(Self::game_hours(current_tick) / hours_per_day + 1)
    }

    /// 根据 tick 获取当前天气显示文本
    ///
    /// 天气 key 从 season.weather_pool 确定性选择，
    /// 显示文本从 display_messages.yaml 的 WeatherConfig 读取。
    /// 动态查找：先从 weather 字段匹配已知 key，再从 weather_events HashMap 查找，
    /// 最后 fallback 到 sunny。
    pub fn get_weather(current_tick: i64) -> Option<String> {
        let weather_key = Self::get_weather_key(current_tick);

        let registry = registry_or_error().ok()?;
        let config = &registry.get().display_messages;
        let weather_config = &config.weather;

        // 动态查找：匹配已知固定字段
        let result = match weather_key.as_str() {
            "sunny" => weather_config.sunny.clone(),
            "cloudy" => weather_config.cloudy.clone(),
            "rainy" => weather_config.rainy.clone(),
            "stormy" => weather_config.stormy.clone(),
            // 扩展：从 weather_events HashMap 查找未知天气类型
            other => config
                .weather_events
                .get(other)
                .cloned()
                .unwrap_or_else(|| weather_config.sunny.clone()),
        };
        Some(result)
    }

    /// 获取格式化的时间显示，用于广播
    pub fn get_time_display(current_tick: i64) -> Option<TimeDisplay> {
        let config = Self::get_config()?;
        let hours_per_day = config.hours_per_day as i64;
        if hours_per_day <= 0 {
            return None;
        }

        let game_hours = Self::game_hours(current_tick);
        let hour_of_day = game_hours % hours_per_day;
        let day_of_season = (game_hours / hours_per_day) % config.days_per_season.max(1) as i64;

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

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_jianghu_protocol::{CalendarConfig, WorldTime, game_day_from_world_time};

    /// game_day 与历史内联公式（chronicle 旧式单级整除、compute_game_time 同构线性数学）
    /// 的等价性钉死，防止单侧修改公式造成"时代判定日"与"统计/展示日"漂移。
    #[test]
    fn game_day_matches_linear_formulas() {
        crate::game_data::init_test_registry();

        // 测试注册表不写 time.json → fallback_time_data（tph=1, hpd=24, dps=10, spy=4）；
        // game_rules 的 real_seconds_per_tick=60 与 FALLBACK_REAL_SECONDS_PER_TICK 一致。
        let rspt = FALLBACK_REAL_SECONDS_PER_TICK;
        let (tph, hpd, dps, spy) = (1i64, 24, 10, 4);
        let rspgd = rspt * tph * hpd;

        let cal = CalendarConfig {
            days_per_season: dps as u32,
            seasons_per_year: spy as u32,
        };

        for tick in [
            0i64,
            1,
            rspt - 1,
            rspt,
            rspgd - 1,
            rspgd,
            rspgd * 7 + 123,
            86_400 * 400,
        ] {
            // 1) chronicle 旧公式（整除单级 = 两级，正除数下恒等）
            let legacy = tick / rspgd + 1;
            assert_eq!(TimeRegistry::game_day(tick), legacy, "tick={tick}");

            // 2) protocol 线性日（compute_game_time 同构：game_hours → y/m/d → game_day_from_world_time）
            let gh = tick / (rspt * tph);
            let hours_per_year = spy * dps * hpd;
            let hours_per_month = dps * hpd;
            let year = 1 + gh / hours_per_year;
            let rem_year = gh % hours_per_year;
            let month = 1 + rem_year / hours_per_month;
            let day = 1 + rem_year % hours_per_month / hpd;
            let wt = WorldTime {
                year: year as i32,
                month: month as i32,
                day: day as i32,
                hour: 0,
                minute: 0,
                second: 0,
                weather: String::new(),
            };
            assert_eq!(
                TimeRegistry::game_day(tick),
                game_day_from_world_time(&wt, &cal),
                "tick={tick}"
            );
        }
    }

    /// try_game_day 与 game_day 在配置可用时必须一致（infallible 包装 fail-fast 变体）；
    /// 配置缺失的 None 路径依赖全局注册表状态，无法在本测试二进制内隔离验证。
    #[test]
    fn try_game_day_agrees_with_game_day_when_config_present() {
        crate::game_data::init_test_registry();
        // 测试注册表 time 配置（loader 默认 tph=1, hpd=24）
        for tick in [0i64, 59, 60, 1440, 1440 * 30 + 7, 86_400 * 365] {
            assert_eq!(
                TimeRegistry::try_game_day(tick).expect("配置已初始化应返回 Some"),
                TimeRegistry::game_day(tick),
                "tick={tick}"
            );
        }
    }
}
