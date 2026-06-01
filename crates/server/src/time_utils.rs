// ============================================================================
// 时间格式化工具
// ============================================================================
//
// 服务端统一的时间格式化入口。所有前端展示的游戏内时间字符串
// 必须通过此模块生成，避免前端重复实现时间转换逻辑。
//
// 数据驱动：从 TimeRegistry 读取时间参数，与 compute_game_time 使用相同配置。
// ============================================================================

use crate::game_data::registry::TimeRegistry;
use cyber_jianghu_protocol::WorldTime;

/// 将游戏日（从1开始）转换为中文日期字符串
///
/// 格式："一七三四年元月四日"（不含"天道历"前缀）
/// 使用与 `WorldTime::to_chinese()` 相同的月份命名方案（元月/二月/.../十月）。
/// 天数 10 输出为"十"而非"一零"。
///
/// 数据驱动：从 TimeRegistry 读取 days_per_season, seasons_per_year
pub fn game_day_to_chinese(game_day: i64) -> String {
    let config = TimeRegistry::get_config();
    match config {
        Some(cfg) => {
            let days_per_season = cfg.days_per_season;
            let seasons_per_year = cfg.seasons_per_year;
            game_day_to_chinese_with_config(game_day, days_per_season, seasons_per_year)
        }
        None => game_day_to_chinese_with_config(game_day, 10, 4),
    }
}

/// 使用显式参数的游戏日格式化（纯函数，可测试）
fn game_day_to_chinese_with_config(
    game_day: i64,
    days_per_season: i32,
    seasons_per_year: i32,
) -> String {
    let days_per_season = days_per_season as i64;
    let seasons_per_year = seasons_per_year as i64;
    let days_per_year = seasons_per_year * days_per_season;

    let gd0 = game_day - 1;
    let year = 1 + (gd0 / days_per_year) as i32;
    let month = 1 + ((gd0 % days_per_year) / days_per_season) as i32;
    let day = 1 + (gd0 % days_per_season) as i32;

    format_chinese_date(year, month, day)
}

/// 格式化为中文日期：{yearChinese}年{monthName}{dayChinese}日
///
/// 与 `WorldTime::to_chinese()` 格式对齐，但没有"天道历"前缀和时辰部分。
/// 月份名采用中文传统命名：元月/二月/三月/四月（五~十/冬月/腊月）。
fn format_chinese_date(year: i32, month: i32, day: i32) -> String {
    let year_str = cyber_jianghu_protocol::digit_to_chinese(year);
    let month_str = match month {
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
        _ => return format!("第{}天{:02}:{:02}", day, 0, 0),
    };
    let day_str = day_to_chinese(day);
    format!("{}年{}{}日", year_str, month_str, day_str)
}

/// 天数（1-10）转中文
///
/// 10 → "十"（非 digit_to_chinese 的逐位 "一零"）
/// 与前端 dayToChinese() 保持一致。
fn day_to_chinese(day: i32) -> String {
    match day {
        0 => "零".to_string(),
        1 => "一".to_string(),
        2 => "二".to_string(),
        3 => "三".to_string(),
        4 => "四".to_string(),
        5 => "五".to_string(),
        6 => "六".to_string(),
        7 => "七".to_string(),
        8 => "八".to_string(),
        9 => "九".to_string(),
        10 => "十".to_string(),
        _ => cyber_jianghu_protocol::digit_to_chinese(day),
    }
}

pub fn parse_world_time_json(json_str: Option<&str>) -> Option<WorldTime> {
    let s = json_str?;
    serde_json::from_str::<WorldTime>(s).ok()
}

pub fn world_time_json_to_game_day(json_str: Option<&str>) -> i64 {
    let Some(wt) = parse_world_time_json(json_str) else {
        return 0;
    };
    let config = TimeRegistry::get_config();
    let (days_per_season, seasons_per_year) = match config {
        Some(cfg) => (cfg.days_per_season as i64, cfg.seasons_per_year as i64),
        None => (10, 4),
    };
    let days_per_year = days_per_season * seasons_per_year;
    (wt.year as i64 - 1) * days_per_year + (wt.month as i64 - 1) * days_per_season + wt.day as i64
}

pub fn world_time_json_to_chinese(json_str: Option<&str>) -> Option<String> {
    parse_world_time_json(json_str).map(|wt| wt.to_chinese())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_game_day_1() {
        // game_day=1, days_per_season=10, seasons_per_year=4 → year=1, month=1, day=1
        assert_eq!(game_day_to_chinese_with_config(1, 10, 4), "一年元月一日");
    }

    #[test]
    fn test_game_day_10() {
        // game_day=10 → year=1, month=1, day=10 → "十日" 非 "一零日"
        assert_eq!(game_day_to_chinese_with_config(10, 10, 4), "一年元月十日");
    }

    #[test]
    fn test_game_day_11() {
        // game_day=11 → year=1, month=2, day=1
        assert_eq!(game_day_to_chinese_with_config(11, 10, 4), "一年二月一日");
    }

    #[test]
    fn test_game_day_40() {
        // game_day=40 → year=1, month=4, day=10
        assert_eq!(game_day_to_chinese_with_config(40, 10, 4), "一年四月十日");
    }

    #[test]
    fn test_game_day_41() {
        // game_day=41 → year=2, month=1, day=1
        assert_eq!(game_day_to_chinese_with_config(41, 10, 4), "二年元月一日");
    }

    #[test]
    fn test_game_day_1734() {
        // year=44, month=2, day=4
        assert_eq!(
            game_day_to_chinese_with_config(1734, 10, 4),
            "四四年二月四日"
        );
    }

    #[test]
    fn test_custom_config() {
        // 不同时间配置测试 days_per_season=15, seasons_per_year=3
        assert_eq!(game_day_to_chinese_with_config(1, 15, 3), "一年元月一日");
        assert_eq!(game_day_to_chinese_with_config(16, 15, 3), "一年二月一日");
        assert_eq!(game_day_to_chinese_with_config(46, 15, 3), "二年元月一日");
    }
}
