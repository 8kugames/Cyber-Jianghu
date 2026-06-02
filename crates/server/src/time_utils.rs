// ============================================================================
// WorldTime JSON 反序列化辅助
// ============================================================================
//
// 用途：服务端数据库以 JSON 形式存储 agent 端 WorldTime 快照，
// 本模块提供从 JSON 字符串反序列化为强类型 WorldTime 的辅助函数。
//
// 时间格式化逻辑统一在协议层（cyber_jianghu_protocol::WorldTime::to_chinese、
// game_day_to_chinese），本模块不做任何日期格式拼接，避免重复造轮子。
// ============================================================================

use crate::game_data::registry::TimeRegistry;
use cyber_jianghu_protocol::{WorldTime, game_day_from_world_time};

fn parse_world_time_json(json_str: Option<&str>) -> Option<WorldTime> {
    let s = json_str?;
    serde_json::from_str::<WorldTime>(s).ok()
}

pub fn world_time_json_to_game_day(json_str: Option<&str>) -> i64 {
    let Some(wt) = parse_world_time_json(json_str) else {
        return 0;
    };
    TimeRegistry::get_calendar_config()
        .map(|cal| game_day_from_world_time(&wt, &cal))
        .unwrap_or(0)
}

pub fn world_time_json_to_chinese(json_str: Option<&str>) -> String {
    parse_world_time_json(json_str)
        .map(|wt| wt.to_chinese())
        .unwrap_or_else(|| "-".to_string())
}
