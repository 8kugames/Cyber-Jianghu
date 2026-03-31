//! ConfigWatcher 文件监听测试
//!
//! 测试 ConfigWatcher 的文件变更检测能力

use cyber_jianghu_agent::infra::api::ConfigWatcher;
use std::path::PathBuf;
use tempfile::NamedTempFile;
use tokio::time::{Duration, timeout};

#[tokio::test]
async fn test_config_watcher_detects_changes() {
    let temp_file = NamedTempFile::new().unwrap();
    let config_path = PathBuf::from(temp_file.path());

    let watcher = ConfigWatcher::new(config_path.clone()).unwrap();
    let mut rx = watcher.subscribe();

    // 写入内容触发变更
    tokio::fs::write(&config_path, b"test: value")
        .await
        .unwrap();

    // 等待通知
    let result = timeout(Duration::from_secs(2), rx.recv()).await;
    assert!(result.is_ok(), "未收到文件变更通知");
}

#[tokio::test]
async fn test_config_watcher_multiple_subscribers() {
    let temp_file = NamedTempFile::new().unwrap();
    let config_path = PathBuf::from(temp_file.path());

    let watcher = ConfigWatcher::new(config_path.clone()).unwrap();
    let mut rx1 = watcher.subscribe();
    let mut rx2 = watcher.subscribe();

    // 写入内容触发变更
    tokio::fs::write(&config_path, b"test: value")
        .await
        .unwrap();

    // 两个订阅者都应该收到通知
    let result1 = timeout(Duration::from_secs(2), rx1.recv()).await;
    let result2 = timeout(Duration::from_secs(2), rx2.recv()).await;

    assert!(result1.is_ok(), "订阅者1未收到文件变更通知");
    assert!(result2.is_ok(), "订阅者2未收到文件变更通知");
}

#[tokio::test]
async fn test_config_watcher_multiple_changes() {
    let temp_file = NamedTempFile::new().unwrap();
    let config_path = PathBuf::from(temp_file.path());

    let watcher = ConfigWatcher::new(config_path.clone()).unwrap();
    let mut rx = watcher.subscribe();

    // 第一次变更
    tokio::fs::write(&config_path, b"test: value1")
        .await
        .unwrap();
    let result1 = timeout(Duration::from_secs(2), rx.recv()).await;
    assert!(result1.is_ok(), "未收到第一次文件变更通知");

    // 第二次变更
    tokio::fs::write(&config_path, b"test: value2")
        .await
        .unwrap();
    let result2 = timeout(Duration::from_secs(2), rx.recv()).await;
    assert!(result2.is_ok(), "未收到第二次文件变更通知");
}

#[test]
fn test_config_watcher_creation() {
    let temp_file = NamedTempFile::new().unwrap();
    let config_path = PathBuf::from(temp_file.path());

    let watcher = ConfigWatcher::new(config_path);
    assert!(watcher.is_ok(), "ConfigWatcher 创建失败");
}

#[test]
fn test_config_watcher_subscribe_before_write() {
    let temp_file = NamedTempFile::new().unwrap();
    let config_path = PathBuf::from(temp_file.path());

    let watcher = ConfigWatcher::new(config_path.clone()).unwrap();
    let _rx = watcher.subscribe(); // 先订阅

    // 然后写入（确保订阅不会阻塞写入）
    std::fs::write(&config_path, b"test: value").unwrap();
}
