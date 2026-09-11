// ============================================================================
// 游戏时换算（自 broadcaster.rs 拆分）
// ============================================================================
//
// tick→游戏时/游戏日/年月日的换算真源在 TimeRegistry
// （game_data/registry/time_registry.rs）：game_hours / game_day /
// game_datetime。本文件仅保留 broadcaster 层的 (1,1,1,0) 回落包装。
// ============================================================================

/// 从 tick_id（秒数）计算游戏时间
///
/// 年月日展开真源 = TimeRegistry::game_datetime；配置缺失/非法时回落 (1,1,1,0)。
pub(super) fn compute_game_time(tick_id: i64) -> (i32, i32, i32, i32) {
    crate::game_data::registry::time_registry::TimeRegistry::game_datetime(tick_id)
        .unwrap_or((1, 1, 1, 0))
}
