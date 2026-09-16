//! scheduler 模块单测（自 scheduler.rs 外移，内容未改）

use super::*;
use chrono::FixedOffset;
use chrono::{Datelike, NaiveDate, TimeZone, Timelike};
use std::io::Write;

/// 验证：`read_file_metadata_for_hot_reload` 在文件不存在时返回 Ok(None)，
/// 而不是 Err。约定：NotFound = 无事可做（正常 skip），不要混入"真错"路径。
#[test]
fn test_read_file_metadata_for_hot_reload_returns_none_for_missing_file() {
    let dir = std::env::temp_dir().join(format!("scheduler_test_missing_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let missing = dir.join("does_not_exist.yaml");

    let result = read_file_metadata_for_hot_reload(&missing).expect("NotFound must not be Err");
    assert!(result.is_none(), "缺失文件必须返回 Ok(None)，但返回了 Some");

    let _ = std::fs::remove_dir_all(&dir);
}

/// 验证：文件存在时返回 Ok(Some(modified))。
#[test]
fn test_read_file_metadata_for_hot_reload_returns_some_for_existing_file() {
    let dir =
        std::env::temp_dir().join(format!("scheduler_test_existing_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("actions.yaml");
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(f, "version: '1.0'").unwrap();
    drop(f);

    let result =
        read_file_metadata_for_hot_reload(&path).expect("existing file metadata must succeed");
    assert!(
        result.is_some(),
        "已存在文件必须返回 Ok(Some(modified))，但返回了 None"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// 验证：NotFound 之外的 IO 错（如路径含 NUL 字节）必须返回 Err，
/// 不能吞掉。约定：除 NotFound 外的 IO 错 = 真错（权限/磁盘/文件锁/路径非法），
/// 必须显式冒泡让 caller 决策。
#[test]
fn test_read_file_metadata_for_hot_reload_returns_err_for_invalid_path() {
    // NUL 字节在 Linux 上是 InvalidInput，不是 NotFound。
    // 这是测试"非 NotFound 错误必须冒泡"的最便携方式（不依赖 chmod / 平台权限）。
    let invalid = std::path::Path::new("actions\0bad.yaml");

    let result = read_file_metadata_for_hot_reload(invalid);
    assert!(
        result.is_err(),
        "非 NotFound IO 错必须返回 Err（warn + 冒泡），\
         而非吞掉返回 Ok(None)。\
         当前 is_ok={}  is_none={}",
        result.is_ok(),
        matches!(result, Ok(None))
    );
}

/// 测试东八区时间解析
///
/// 验证 start_date: "2026-03-03" 被正确解析为北京时间 00:00:00
#[test]
fn test_utc8_game_epoch() {
    // 解析日期字符串
    let start_date_str = "2026-03-03";
    let date = NaiveDate::parse_from_str(start_date_str, "%Y-%m-%d").unwrap();

    // 使用东八区（UTC+8）时间
    let offset = FixedOffset::east_opt(8 * 3600).unwrap();
    let datetime = date.and_hms_opt(0, 0, 0).unwrap();
    let datetime_with_tz = datetime.and_local_timezone(offset).single().unwrap();

    // 获取 Unix 时间戳
    let timestamp = datetime_with_tz.timestamp();

    // 验证：北京时间 2026-03-03 00:00:00 = UTC 2026-03-02 16:00:00
    // 预期的 UTC 时间戳
    let expected_utc = NaiveDate::from_ymd_opt(2026, 3, 2)
        .unwrap()
        .and_hms_opt(16, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp();

    assert_eq!(
        timestamp, expected_utc,
        "北京时间 2026-03-03 00:00:00 应该等于 UTC 2026-03-02 16:00:00"
    );

    // 验证具体数值
    // 2026-03-02 16:00:00 UTC 的 Unix 时间戳
    // 通过在线工具验证：https://www.unixtimestamp.com/
    // 2026-03-03 00:00:00 UTC+8 = 2026-03-02 16:00:00 UTC = 1772467200
    assert_eq!(timestamp, 1772467200, "时间戳应该等于 1772467200");
}

/// 测试 tick_id 计算（秒级秒数）
///
/// 验证 tick_id = now - game_epoch（秒级秒数）
#[test]
fn test_tick_id_calculation() {
    let start_date_str = "2026-03-03";
    let date = NaiveDate::parse_from_str(start_date_str, "%Y-%m-%d").unwrap();
    let offset = FixedOffset::east_opt(8 * 3600).unwrap();
    let datetime = date.and_hms_opt(0, 0, 0).unwrap();
    let game_epoch = datetime
        .and_local_timezone(offset)
        .single()
        .unwrap()
        .timestamp();

    // tick_id = now - game_epoch（秒级秒数）
    // 在北京时间 2026-03-03 00:00:00，tick_id 应该是 0
    let tick_at_epoch = game_epoch - game_epoch;
    assert_eq!(tick_at_epoch, 0, "纪元时刻的 tick_id 应该是 0");

    // 在北京时间 2026-03-03 00:01:00（1分钟后），tick_id 应该是 60
    let one_minute_later = game_epoch + 60;
    let tick_after_1min = one_minute_later - game_epoch;
    assert_eq!(tick_after_1min, 60, "1分钟后的 tick_id 应该是 60");

    // 在北京时间 2026-03-03 01:00:00（1小时后），tick_id 应该是 3600
    let one_hour_later = game_epoch + 3600;
    let tick_after_1hour = one_hour_later - game_epoch;
    assert_eq!(tick_after_1hour, 3600, "1小时后的 tick_id 应该是 3600");
}

/// 测试时间戳转换的一致性
///
/// 验证从时间戳反向转换回日期时间的正确性
#[test]
fn test_timestamp_roundtrip() {
    let start_date_str = "2026-03-03";
    let date = NaiveDate::parse_from_str(start_date_str, "%Y-%m-%d").unwrap();
    let offset = FixedOffset::east_opt(8 * 3600).unwrap();
    let datetime = date.and_hms_opt(0, 0, 0).unwrap();
    let datetime_with_tz = datetime.and_local_timezone(offset).single().unwrap();

    let timestamp = datetime_with_tz.timestamp();

    // 从时间戳反向转换
    let reversed = offset.timestamp_opt(timestamp, 0).single().unwrap();

    // 验证年月日时分秒一致
    assert_eq!(reversed.year(), 2026);
    assert_eq!(reversed.month(), 3);
    assert_eq!(reversed.day(), 3);
    assert_eq!(reversed.hour(), 0);
    assert_eq!(reversed.minute(), 0);
    assert_eq!(reversed.second(), 0);
}
