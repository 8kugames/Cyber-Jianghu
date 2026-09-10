// ============================================================================
// 游戏时换算（自 broadcaster.rs 拆分）
// ============================================================================
//
// tick→游戏时/游戏日的换算真源在 TimeRegistry::game_hours / game_day
// （game_data/registry/time_registry.rs）；游戏日直接调 TimeRegistry::game_day；
// 本文件仅保留 compute_game_time 的年月日时展开。
// ============================================================================

/// 从 tick_id（秒数）计算游戏时间
///
/// 数据驱动：从 TimeRegistry 和 GameRules 读取时间参数
/// 返回 (year, month, day, hour)
pub(super) fn compute_game_time(tick_id: i64) -> (i32, i32, i32, i32) {
    let time_config = crate::game_data::registry::TimeRegistry::get_config();
    if let Some(config) = time_config {
        let hours_per_day = config.hours_per_day as i64;
        let days_per_season = config.days_per_season as i64;
        let seasons_per_year = config.seasons_per_year as i64;
        if hours_per_day <= 0 || days_per_season <= 0 || seasons_per_year <= 0 {
            tracing::warn!(
                "time.yaml 参数非法（hours_per_day={} days_per_season={} seasons_per_year={}），使用默认时间",
                hours_per_day,
                days_per_season,
                seasons_per_year
            );
            return (1, 1, 1, 0);
        }
        let days_per_year = seasons_per_year * days_per_season;

        // 换算真源：TimeRegistry::game_hours（禁止内联复制公式）
        let game_hours = crate::game_data::registry::TimeRegistry::game_hours(tick_id);

        let hours_per_year = days_per_year * hours_per_day;
        let hours_per_month = days_per_season * hours_per_day;

        let year = 1 + (game_hours / hours_per_year) as i32;
        let rem_after_year = game_hours % hours_per_year;
        let month = 1 + (rem_after_year / hours_per_month) as i32;
        let rem_after_month = rem_after_year % hours_per_month;
        let day = 1 + (rem_after_month / hours_per_day) as i32;
        let hour = (rem_after_month % hours_per_day) as i32;

        (year, month, day, hour)
    } else {
        (1, 1, 1, 0)
    }
}
