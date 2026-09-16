// ============================================================================
// 通道接收辅助（超时读 / WorldState 发送失败处理）
// ============================================================================

/// 会让 Agent 在 LLM 卡住时基于过期状态继续决策。
/// 在 mpsc receiver 上接收下一条消息，带超时。
/// 约定（**根因级修复**，消除 `wait_for_execution_result` timeout 静默返回空 Vec）：
/// - `Ok(Some(T))`：在 timeout 内收到一条消息
/// - `Err("recv timeout after ...")`：超时无消息，**caller 必须显式处理**
/// - `Err("recv channel closed")`：sender 已 drop，通道关闭
///
/// 之前 timeout 被吞为 `Ok(Vec::new())`——caller 误以为"无响应继续"而非"timeout 该重试/失败"，
pub(super) async fn recv_with_timeout<T>(
    rx: &mut tokio::sync::mpsc::Receiver<T>,
    timeout: std::time::Duration,
) -> anyhow::Result<Option<T>> {
    match tokio::time::timeout(timeout, rx.recv()).await {
        Ok(Some(v)) => Ok(Some(v)),
        Ok(None) => Err(anyhow::anyhow!("recv channel closed")),
        Err(_) => Err(anyhow::anyhow!("recv timeout after {timeout:?}")),
    }
}

/// 处理 worldstate_tx.send 失败——仅分级记日志，**绝不清空 sender**
///
/// watch::Sender::send 在零 receiver 时返回 SendError，但这是**正常的瞬时态**，不能据此
/// 清空 sender：
/// - lifecycle 主循环用 `select!` 包裹 `receive_world_state()`，每次别的分支胜出，该
///   future 被 drop → 其 watch::Receiver 被 drop → 出现零 receiver 窗口；
/// - server 在连接后会**立即推送 initial WorldState**（见 server
///   websocket/handler.rs `build_initial_world_state`），此刻 lifecycle 尚在 reconnect
///   注册流程、未 subscribe，同样处于零 receiver 状态。
///
/// 若在此处清空 `worldstate_tx`，lifecycle 下一次 `receive_world_state()` 会读到 None
/// 并误判 "Not connected to server" → 触发 reconnect → 重连后再被清 → **WS 重连风暴**
/// （~3Hz 死循环、backoff 永不升级、零 intent 提交）。这是历史引入、经联调复现确认的回归。
///
/// 修法：仅按 `is_closed()` 分级记日志（零 receiver → debug，已够抑制原 1300+/24h warn
/// 噪音；罕见半失败 → warn 保留诊断），**保留 sender**——receiver 回归后 send 自然恢复。
pub(super) fn handle_worldstate_send_failure<T>(
    tx: &tokio::sync::watch::Sender<T>,
    context: &str,
    err: tokio::sync::watch::error::SendError<T>,
) {
    if tx.is_closed() {
        tracing::debug!(
            "worldstate_tx.send 失败 [{}]：暂无 receiver（select! 间隙或初始推送），保留 sender",
            context
        );
        let _ = err;
    } else {
        tracing::warn!(
            "worldstate_tx.send 失败 [{}]（receiver 仍在但 send 失败）：{:?}",
            context,
            err
        );
    }
}
