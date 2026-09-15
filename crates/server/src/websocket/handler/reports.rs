// ============================================================================
// 周期性上报处理（三魂循环 / 每日摘要 / 关系快照）
// ============================================================================

use super::*;

/// 处理三魂循环元数据上报
///
/// Agent 在 intent 发送后通过 WebSocket SoulCycleReport 消息上报三魂循环详情。
/// Server 将元数据关联到同一 tick 的 agent_action_logs 记录。
pub(super) async fn handle_soul_cycle_report(
    device_id: uuid::Uuid,
    msg_agent_id: Option<uuid::Uuid>,
    tick_id: i64,
    pipe_seq: i32,
    metadata: &SoulCycleMetadata,
    state: &Arc<crate::state::AppState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 确定最终的 agent_id（与 handle_intent 相同逻辑：比较 device_id）
    let agent_id = match msg_agent_id {
        Some(id) if id != uuid::Uuid::nil() => {
            let owner_device_id: Option<uuid::Uuid> =
                sqlx::query_scalar("SELECT device_id FROM agents WHERE agent_id = $1")
                    .bind(id)
                    .fetch_optional(&state.db_pool)
                    .await
                    .context("查询 Agent 归属失败")?;

            match owner_device_id {
                Some(owner) if owner == device_id => id,
                Some(_) => {
                    warn!(
                        "SoulCycleReport: Agent ownership mismatch: agent={}, device={}",
                        id, device_id
                    );
                    return Err("无权操作此角色".into());
                }
                None => return Err("Agent 不存在".into()),
            }
        }
        _ => {
            // nil / None → 通过 device_id 查找当前 agent
            match crate::db::get_agent_by_device_id(&state.db_pool, device_id).await {
                Ok(Some(agent)) => agent.agent_id,
                Ok(None) => return Err("无关联角色".into()),
                Err(e) => return Err(format!("查询角色失败: {}", e).into()),
            }
        }
    };

    debug!(
        "收到三魂循环元数据：agent={}, tick={}, attempts={}",
        agent_id,
        tick_id,
        metadata.cycles.len()
    );

    // 将 metadata 序列化为 JSON
    let metadata_json = serde_json::to_value(metadata).context("序列化三魂循环元数据失败")?;

    // 更新 agent_action_logs 表
    if let Err(e) = crate::db::update_soul_cycle_metadata(
        &state.db_pool,
        agent_id,
        tick_id,
        pipe_seq,
        &metadata_json,
    )
    .await
    {
        warn!(
            "写入三魂循环元数据失败: agent={}, tick={}, err={:#}",
            agent_id, tick_id, e
        );
    }

    Ok(())
}

/// 处理每日 LLM 日志摘要上报
///
/// Agent 通过 WebSocket DailySummary 消息上报游戏日结束时的 LLM 事件摘要。
/// Server 注入 created_at 时间戳（服务器权威时间），然后 UPSERT 到数据库。
pub(super) async fn handle_daily_summary(
    device_id: uuid::Uuid,
    game_day: i64,
    summary: &str,
    state: &Arc<crate::state::AppState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 通过 device_id 查找当前 agent
    let agent_id = match crate::db::get_agent_by_device_id(&state.db_pool, device_id).await {
        Ok(Some(agent)) => agent.agent_id,
        Ok(None) => return Err("无关联角色".into()),
        Err(e) => return Err(format!("查询角色失败: {}", e).into()),
    };

    // Server 注入时间戳（服务器权威时间，非客户端）
    let created_at = chrono::Utc::now().timestamp_millis();

    debug!(
        "收到每日摘要：agent_id={}, game_day={}, summary_len={}",
        agent_id,
        game_day,
        summary.len()
    );

    if let Err(e) = crate::db::upsert_agent_daily_summary(
        &state.db_pool,
        agent_id,
        game_day,
        summary,
        created_at,
    )
    .await
    {
        error!(
            "写入每日摘要失败: agent_id={}, game_day={}, err={}",
            agent_id, game_day, e
        );
        return Err(format!("写入每日摘要失败: {}", e).into());
    }

    info!(
        "每日摘要已存储: agent_id={}, game_day={}",
        agent_id, game_day
    );

    Ok(())
}

/// 校验 RelationshipSnapshot 消息归属：msg_agent_id 必须与 device 当前绑定的 agent 一致。
///
/// 这是纯函数（无 DB / 无 IO），从 handle_relationship_snapshot 抽出以便单测。
/// 返回 Ok(()) 表示归属匹配，Err 表示拒绝（防越权改写他人关系图谱）。
pub(crate) fn validate_relationship_snapshot_ownership(
    resolved_agent_id: uuid::Uuid,
    msg_agent_id: uuid::Uuid,
) -> Result<(), String> {
    if msg_agent_id != resolved_agent_id {
        return Err(format!(
            "关系快照 agent_id 不匹配: expected {resolved_agent_id}, got {msg_agent_id}"
        ));
    }
    Ok(())
}

/// 处理关系图谱全量快照上报
///
/// Agent 通过 WebSocket RelationshipSnapshot 消息上报游戏日结束时的完整关系列表。
/// Server 全量覆盖（DELETE+INSERT，天然幂等），注入 synced_at（服务器权威时间）。
///
/// 归属校验：消息内 agent_id 必须与 device_id 解析出的当前 agent 一致，
/// 否则拒绝（防止越权改写他人关系图谱）。
pub(super) async fn handle_relationship_snapshot(
    device_id: uuid::Uuid,
    msg_agent_id: uuid::Uuid,
    game_day: i64,
    relationships: &[cyber_jianghu_protocol::types::RelationshipMemory],
    state: &Arc<crate::state::AppState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 通过 device_id 查找当前 agent
    let agent_id = match crate::db::get_agent_by_device_id(&state.db_pool, device_id).await {
        Ok(Some(agent)) => agent.agent_id,
        Ok(None) => return Err("无关联角色".into()),
        Err(e) => return Err(format!("查询角色失败: {}", e).into()),
    };

    // 归属校验：消息内 agent_id 必须与连接归属的 agent 一致
    if let Err(reason) = validate_relationship_snapshot_ownership(agent_id, msg_agent_id) {
        warn!(
            "关系快照归属校验失败: device={} resolved={} msg={}，拒绝写入 ({})",
            device_id, agent_id, msg_agent_id, reason
        );
        return Err(reason.into());
    }

    // Server 注入时间戳（服务器权威时间）
    let synced_at = chrono::Utc::now().timestamp_millis();

    debug!(
        "收到关系快照: agent_id={}, game_day={}, relationships={}",
        agent_id,
        game_day,
        relationships.len()
    );

    if let Err(e) = crate::db::upsert_relationship_snapshot(
        &state.db_pool,
        agent_id,
        game_day,
        relationships,
        synced_at,
    )
    .await
    {
        error!(
            "写入关系快照失败: agent_id={}, game_day={}, err={}",
            agent_id, game_day, e
        );
        return Err(format!("写入关系快照失败: {}", e).into());
    }

    info!(
        "关系快照已存储: agent_id={}, game_day={}, count={}",
        agent_id,
        game_day,
        relationships.len()
    );

    Ok(())
}
