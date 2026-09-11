// ============================================================================
// 年龄与寿龄换算（自 decay.rs 拆分，原文件超 800 行上限）
// ============================================================================
//
// tick→游戏岁的换算真源 = TimeRegistry::game_hours（禁止内联复制公式）。
// ============================================================================

/// 从 birth_tick 和 current_tick 计算角色年龄（游戏年）
///
/// tick_id 是秒级时间戳；tick→游戏时换算真源 = TimeRegistry::game_hours
/// （禁止内联复制公式），年龄 = game_hours(age_ticks) / hours_per_year
pub fn compute_age_years(birth_tick: i64, current_tick: i64) -> i64 {
    use crate::game_data::registry::TimeRegistry;

    let age_ticks = current_tick - birth_tick;
    if age_ticks <= 0 {
        return 0;
    }

    // 配置缺失时 fail-soft 返回 0（不衰减），公式真源 = TimeRegistry::game_hours
    let Some(time_config) = TimeRegistry::get_config() else {
        return 0;
    };

    let hours_per_year = time_config.hours_per_day as i64
        * time_config.days_per_season as i64
        * time_config.seasons_per_year as i64;
    if hours_per_year <= 0 {
        return 0;
    }
    TimeRegistry::game_hours(age_ticks) / hours_per_year
}

/// 计算 starting_age 对应的 tick 偏移量（用于重生时设置 birth_tick）
///
/// 使 age 计算结果为 starting_age 岁，而非 0 岁。
/// tick_id 是秒级时间戳，偏移量需包含 real_seconds_per_tick 转换。
pub fn compute_starting_age_ticks() -> i64 {
    let registry = match crate::game_data::registry_or_error() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("decay: registry 读取失败（fallback 0 = 不衰减）：{e:?}");
            return 0;
        }
    };
    let gd = registry.get();
    let starting_age = gd
        .game_rules
        .data
        .lifespan
        .as_ref()
        .map(|l| l.starting_age as i64)
        .unwrap_or(18);

    // 边界检查：starting_age 不能大于 max_age
    let max_age = gd
        .game_rules
        .data
        .lifespan
        .as_ref()
        .map(|l| l.max_age as i64)
        .unwrap_or(80);
    let starting_age = starting_age.min(max_age);

    if starting_age <= 0 {
        return 0;
    }

    if let Some(time_config) = crate::game_data::registry::TimeRegistry::get_config() {
        let ticks_per_hour = time_config.ticks_per_hour as i64;
        let hours_per_year = time_config.hours_per_day as i64
            * time_config.days_per_season as i64
            * time_config.seasons_per_year as i64;

        let real_seconds_per_tick =
            gd.game_rules.data.agent_state.tick.real_seconds_per_tick as i64;
        let real_seconds_per_game_hour = real_seconds_per_tick * ticks_per_hour;
        let real_seconds_per_year = real_seconds_per_game_hour * hours_per_year;
        if real_seconds_per_year > 0 {
            return starting_age * real_seconds_per_year;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    // ============================================================================
    // 年龄计算单元测试
    // ============================================================================

    /// 测试 compute_age_years 的基本计算
    ///
    /// 测试配置: ticks_per_hour=1, real_seconds_per_tick=60,
    /// hours_per_day=24, days_per_season=10, seasons_per_year=4
    /// → real_seconds_per_game_hour = 60 * 1 = 60
    /// → hours_per_year = 24 * 10 * 4 = 960
    /// → real_seconds_per_year = 60 * 960 = 57600
    ///
    /// tick_id 是秒级时间戳，age_ticks / real_seconds_per_game_hour = game_hours
    #[test]
    fn test_compute_age_years_basic() {
        crate::game_data::init_test_registry();

        // 57600 秒差 = 1 游戏年（60 * 960）
        assert_eq!(compute_age_years(0, 57600), 1);
        assert_eq!(compute_age_years(0, 115200), 2);
        assert_eq!(compute_age_years(0, 576000), 10);

        // birth_tick > 0
        assert_eq!(compute_age_years(57600, 115200), 1);
        assert_eq!(compute_age_years(57600, 576000), 9);
    }

    /// 测试 compute_age_years 边界条件
    #[test]
    fn test_compute_age_years_edge_cases() {
        crate::game_data::init_test_registry();

        // birth_tick == current_tick → 0 岁
        assert_eq!(compute_age_years(100, 100), 0);

        // birth_tick > current_tick → 0 岁（还没出生）
        assert_eq!(compute_age_years(200, 100), 0);

        // 不足 1 年 → 0 岁（整数除法截断）
        assert_eq!(compute_age_years(0, 57599), 0);
    }

    /// 测试 compute_age_years 与 compute_starting_age_ticks 的 round-trip 可逆性
    ///
    /// 如果 birth_tick = current_tick - starting_age_ticks，
    /// 则 compute_age_years 应返回 starting_age。
    #[test]
    fn test_age_round_trip() {
        crate::game_data::init_test_registry();

        let starting_ticks = compute_starting_age_ticks();
        // starting_age=18, real_seconds_per_year = 60 * 960 = 57600
        // → starting_ticks = 18 * 57600 = 1036800
        assert_eq!(
            starting_ticks,
            18 * 60 * 960,
            "starting_age=18 应产生 1036800 秒偏移"
        );

        let current_tick = 1036800;
        let birth_tick = current_tick - starting_ticks;

        let age = compute_age_years(birth_tick, current_tick);
        assert_eq!(age, 18, "round-trip 应返回 starting_age=18");
    }
}
