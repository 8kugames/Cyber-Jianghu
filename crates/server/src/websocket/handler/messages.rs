// ============================================================================
// 客户端消息分发（handle_client_message）与 trace 上报
// ============================================================================

use super::*;

// ============================================================================
// 消息处理
// ============================================================================

/// 处理客户端消息
///
/// 根据消息类型进行相应的处理
pub(super) async fn handle_client_message(
    agent_id: &uuid::Uuid,
    device_id: uuid::Uuid,
    msg: ClientMessage,
    state: &Arc<crate::state::AppState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match msg {
        ClientMessage::Intent {
            intent_id,
            tick_id,
            agent_id: msg_agent_id,
            thought_log,
            action_type,
            action_data,
            priority,
            subsequent_intents,
            soul_cycle_metadata,
            chaos_marker,
            dream_marker,
        } => {
            handle_intent(
                *agent_id,
                device_id,
                msg_agent_id,
                intent_id,
                tick_id,
                thought_log,
                action_type,
                action_data,
                priority,
                subsequent_intents,
                soul_cycle_metadata,
                chaos_marker,
                dream_marker,
                state,
            )
            .await
        }
        ClientMessage::Dialogue { message } => {
            handle_dialogue_message(*agent_id, message, state).await
        }
        ClientMessage::SoulCycleReport {
            tick_id,
            agent_id: msg_agent_id,
            pipe_seq,
            metadata,
        } => {
            handle_soul_cycle_report(device_id, msg_agent_id, tick_id, pipe_seq, &metadata, state)
                .await
        }
        ClientMessage::DailySummary { game_day, summary } => {
            handle_daily_summary(device_id, game_day, &summary, state).await
        }
        ClientMessage::RelationshipSnapshot {
            agent_id: msg_agent_id,
            game_day,
            relationships,
        } => {
            handle_relationship_snapshot(device_id, msg_agent_id, game_day, &relationships, state)
                .await
        }
        ClientMessage::TraceReport { traces } => {
            handle_trace_report(device_id, &traces, state).await
        }
    }
}

/// 处理训练 Trace 上报（agent → server 汇聚）
///
/// agent 端的结构化 LLM 调用 trace（已脱敏）批量回传，server 落盘后
/// 与 reward 同目录树（get_data_dir()），训练导出时按 (agent_id, tick_id) join。
pub(super) async fn handle_trace_report(
    device_id: uuid::Uuid,
    traces: &[cyber_jianghu_protocol::TraceEntry],
    state: &std::sync::Arc<crate::state::AppState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 通过 device_id 查找当前 agent（对齐 handle_daily_summary 模式）
    let agent_id = match crate::db::get_agent_by_device_id(&state.db_pool, device_id).await {
        Ok(Some(agent)) => agent.agent_id,
        Ok(None) => return Err("无关联角色".into()),
        Err(e) => return Err(format!("查询角色失败: {}", e).into()),
    };

    // 落盘到 server 侧 traces/（与 rewards/ 同根目录）
    let traces_dir = crate::paths::get_data_dir().join("traces");
    for entry in traces {
        let soul = &entry.soul_stage;
        // date 用 trace 真实产生时间（Unix 毫秒），非 server 接收时间
        let date = entry
            .wall_clock
            .and_then(|ms| {
                chrono::DateTime::from_timestamp_millis(ms)
                    .map(|dt| dt.format("%Y-%m-%d").to_string())
            })
            .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
        let dir = traces_dir
            .join(format!("soul={}", soul))
            .join(format!("agent={}", agent_id));
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            tracing::error!("[trace] server 创建目录失败: {}", e);
            continue;
        }
        let path = dir.join(format!("date={}.jsonl", date));
        let line = match serde_json::to_string(entry) {
            Ok(s) => s + "\n",
            Err(e) => {
                tracing::warn!("[trace] server 序列化失败: {}", e);
                continue;
            }
        };
        use tokio::io::AsyncWriteExt;
        match tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
        {
            Ok(mut f) => {
                if let Err(e) = f.write_all(line.as_bytes()).await {
                    tracing::warn!("[trace] server 写入失败 {:?}: {}", path, e);
                }
            }
            Err(e) => tracing::warn!("[trace] server 打开文件失败 {:?}: {}", path, e),
        }
    }

    tracing::debug!(
        "[trace] server 收到 {} 条 trace（device={}, agent={}）",
        traces.len(),
        device_id,
        agent_id
    );
    Ok(())
}
