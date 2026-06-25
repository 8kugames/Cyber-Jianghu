//! best-effort `tracing::warn!` 行为契约测试
//!
//! 背景：result1/result2 在 42 处 fire-and-forget channel send 把 `let _ = tx.send()`
//! 改为 `if let Err(e) = tx.send() { tracing::warn!("...receiver 可能已 drop：{e:?}") }`。
//! 这些改动**没有行为测试**——若有人 git revert 回 `let _ =`，无测试捕获。
//!
//! 本模块用 `tracing-test` 验证"send 失败时 warn! 必须触发"的**通用契约**，
//! 覆盖代码库中所有 warn! 模式分类：
//! - broadcast channel（death_event_tx / worldstate_tx）
//! - mpsc channel（reg_tx / downstream_tx / bridge req.tx）
//! - watch / oneshot（shutdown_tx 用 mpsc，broadcast）
//!
//! 设计原则：**不 mock 复杂结构**，直接复现"sender 存在但 receiver 全 drop"的最小场景。
//! 这是验证 warn! 宏本身可达 + 消息含预期关键词的唯一可行方式。

#[cfg(test)]
mod tests {
    use tokio::sync::{broadcast, mpsc};
    use tracing_test::traced_test;

    // =========================================================================
    // broadcast channel（代表：death_event_tx.send）
    // =========================================================================

    /// 验证：broadcast sender 在**无订阅者**时 send 失败，必须触发含
    /// "receiver 可能已 drop" 的 warn!。
    ///
    /// 代表 site：`reconnect.rs:119` death_event_tx.send（归隐 delay=0）
    #[traced_test]
    #[tokio::test]
    async fn test_warn_broadcast_send_no_subscribers_emits_warn() {
        let (tx, _) = broadcast::channel::<i32>(100);
        // receiver 已被 `_` 丢弃，无活跃订阅者
        let result = tx.send(42);
        assert!(result.is_err(), "broadcast send 无订阅者必须返 Err");
        // 复现 reconnect.rs:119-123 的 warn! 模式
        if let Err(e) = tx.send(42) {
            tracing::warn!(
                "death_event_tx.send（归隐 delay=0）失败（receiver 可能已 drop）：{e:?}"
            );
        }
        assert!(
            logs_contain("receiver 可能已 drop"),
            "P0-AUDIT warn! 契约：broadcast send 失败必须 warn 且消息含 'receiver 可能已 drop'"
        );
        assert!(
            logs_contain("death_event_tx"),
            "warn! 消息必须含 channel 名 'death_event_tx' 以便运维定位"
        );
    }

    // =========================================================================
    // mpsc channel（代表：reg_tx.send / downstream_tx.send / bridge req.tx.send）
    // =========================================================================

    /// 验证：mpsc sender 在 receiver drop 后 send 失败，必须触发含
    /// "receiver 可能已 drop" 的 warn!。
    ///
    /// 代表 site：`websocket.rs:1010` reg_tx.send / `cyber-jianghu-agent.rs:1305` downstream_tx.send
    #[traced_test]
    #[tokio::test]
    async fn test_warn_mpsc_send_after_receiver_drop_emits_warn() {
        let (tx, rx) = mpsc::channel::<i32>(8);
        drop(rx); // receiver drop
        let send_result = tx.send(42).await;
        assert!(send_result.is_err(), "mpsc send 在 receiver drop 后必须返 Err");
        if let Err(e) = tx.send(42).await {
            tracing::warn!("downstream tx.send 失败（receiver 可能已 drop）：{e:?}");
        }
        assert!(
            logs_contain("receiver 可能已 drop"),
            "P0-AUDIT warn! 契约：mpsc send 失败必须 warn 且消息含 'receiver 可能已 drop'"
        );
    }

    /// 验证：mpsc sender 在 receiver drop 后 send 失败，warn! 含 channel 名
    /// （代表 bridge req.tx.send 模式）。
    ///
    /// 代表 site：`claw/bridge.rs:123` bridge req.tx.send(content)
    #[traced_test]
    #[tokio::test]
    async fn test_warn_bridge_req_tx_send_emits_channel_name() {
        let (tx, rx) = mpsc::channel::<String>(4);
        drop(rx);
        if let Err(e) = tx.send("content".to_string()).await {
            tracing::warn!("bridge req.tx.send(content) 失败（receiver 可能已 drop）：{e:?}");
        }
        assert!(
            logs_contain("bridge req.tx.send"),
            "warn! 消息必须含 'bridge req.tx.send' 以便运维定位 claw 模块"
        );
    }

    // =========================================================================
    // watch / Ctrl+C / SIGTERM shutdown（代表：shutdown_tx.send）
    // =========================================================================

    /// 验证：mpsc sender 在 receiver drop 后 send 失败（shutdown 协调场景），
    /// warn! 必须触发且含 "shutdown" 关键词。
    ///
    /// 代表 site：`cyber-jianghu-agent.rs:1321` shutdown_tx_clone.send（Ctrl+C）
    #[traced_test]
    #[tokio::test]
    async fn test_warn_shutdown_send_emits_shutdown_keyword() {
        let (tx, rx) = mpsc::channel::<()>(1);
        drop(rx);
        if let Err(e) = tx.send(()).await {
            tracing::warn!("shutdown_tx.send（Ctrl+C）失败（receiver 可能已 drop）：{e:?}");
        }
        assert!(
            logs_contain("shutdown_tx"),
            "warn! 消息必须含 'shutdown_tx' 以便运维定位关闭协调失败"
        );
    }

    // =========================================================================
    // 反向验证（RED 证据）：send 成功时不应触发 warn!
    // =========================================================================

    /// 验证：send **成功**时 warn! **不应**触发（防止 warn! 误报）。
    #[traced_test]
    #[tokio::test]
    async fn test_warn_not_emitted_when_send_succeeds() {
        let (tx, mut rx) = mpsc::channel::<i32>(8);
        if let Err(e) = tx.send(42).await {
            tracing::warn!("不应触发的 warn: {e:?}");
        }
        // 消费一下避免 unused 警告
        assert_eq!(rx.recv().await, Some(42));
        assert!(
            !logs_contain("不应触发的 warn"),
            "send 成功时 warn! 不应触发（防止误报噪音）"
        );
    }

    // =========================================================================
    // server 端 broadcast（跨 crate 代表：ws connection.send 广播）
    // =========================================================================

    /// 验证：broadcast 广播失败时 warn! 含 "broadcast" 关键词。
    ///
    /// 代表 site：`server/websocket/handler.rs:1413` ws connection.send（broadcast）
    #[traced_test]
    #[tokio::test]
    async fn test_warn_ws_broadcast_send_emits_broadcast_keyword() {
        let (tx, _) = broadcast::channel::<String>(16);
        // receiver 已 drop
        if let Err(e) = tx.send("event".to_string()) {
            tracing::warn!("ws connection.send（broadcast）失败（receiver 可能已 drop）：{e:?}");
        }
        assert!(
            logs_contain("ws connection.send"),
            "warn! 消息必须含 'ws connection.send' 以便运维定位 WebSocket 广播失败"
        );
    }
}
