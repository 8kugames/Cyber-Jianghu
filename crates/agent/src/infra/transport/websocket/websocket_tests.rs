//! transport websocket 单测（recv_with_timeout 超时契约）

use super::recv_with_timeout;
use tokio::sync::mpsc;

/// 验证：recv_with_timeout 在 timeout 内**必须返回 Err**，
/// 而非静默返回 Ok(None) / Ok(Some(default))。之前 `wait_for_execution_result`
/// 的 `Err(_) => Ok(Vec::new())` 会让 Agent 在 LLM 卡住时基于过期状态继续决策。
#[tokio::test]
async fn test_recv_with_timeout_returns_err_on_no_message() {
    let (_tx, mut rx) = mpsc::channel::<u32>(1);
    // 不发任何消息，等 timeout
    let result = recv_with_timeout(&mut rx, std::time::Duration::from_millis(50)).await;
    assert!(
        result.is_err(),
        "timeout 必须返回 Err，caller 决定重试/失败。当前 is_ok={}",
        result.is_ok()
    );
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("timeout"),
        "错误消息必须包含 'timeout'，让运维一眼能看出是超时。msg={msg}"
    );
}

/// 验证：recv_with_timeout 在 sender drop 后**必须返回 Err**（"channel closed"），
/// 不能吞为 Ok(None)。
#[tokio::test]
async fn test_recv_with_timeout_returns_err_on_closed_channel() {
    let (tx, mut rx) = mpsc::channel::<u32>(1);
    drop(tx); // sender 关闭
    let result = recv_with_timeout(&mut rx, std::time::Duration::from_millis(50)).await;
    assert!(result.is_err(), "closed channel 必须返回 Err");
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("closed"),
        "错误消息必须包含 'closed'。msg={msg}"
    );
}

/// 验证：recv_with_timeout 在 timeout 内收到消息**必须返回 Ok(Some(v))**。
#[tokio::test]
async fn test_recv_with_timeout_returns_some_on_message() {
    let (tx, mut rx) = mpsc::channel::<u32>(1);
    tx.send(42).await.unwrap();
    let result = recv_with_timeout(&mut rx, std::time::Duration::from_millis(50)).await;
    assert_eq!(result.unwrap(), Some(42), "收到消息必须返回 Ok(Some(42))");
}

/// 回归锁定：receiver drop 后 send 失败时，handle_worldstate_send_failure
/// 必须**只记日志、保留 sender**——清空 sender 会破坏 `receive_world_state()`，引发 WS 重连风暴。
#[tokio::test]
async fn test_handle_worldstate_send_failure_keeps_sender_on_receiver_drop() {
    let (tx, rx) = tokio::sync::watch::channel::<Option<u32>>(None);

    // 模拟零 receiver（select! 间隙 / lifecycle 未 subscribe 时收到初始 WorldState）
    drop(rx);
    assert!(
        tx.is_closed(),
        "前置：零 receiver 时 sender.is_closed() 必须为 true"
    );

    // 零 receiver 时 send 必须失败
    let send_err = tx.send(Some(42)).expect_err("零 receiver 时 send 必须失败");
    // 调用 helper：只记日志，不得清空 sender
    super::handle_worldstate_send_failure(&tx, "test-ctx", send_err);

    // 关键断言：sender 仍存活——subscribe 新 receiver 后 send 必须恢复成功
    let _rx2 = tx.subscribe();
    assert!(
        tx.send(Some(7)).is_ok(),
        "helper 不得清空 sender：subscribe 回归后 send 必须恢复"
    );
}
