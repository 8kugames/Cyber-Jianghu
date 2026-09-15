// ============================================================================
// Intent 处理（含 subsequent intents 原子队列）
// ============================================================================

use super::*;

/// 处理意图上报
///
/// 处理 Agent 提交的 Intent（实时模式：非阻塞入队 IntentWorker）
/// 包含速率限制检查、Agent 存活检查，speak/whisper 即时广播
#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_intent(
    connection_agent_id: uuid::Uuid,
    device_id: uuid::Uuid,
    msg_agent_id: Option<uuid::Uuid>,
    req_intent_id: Option<uuid::Uuid>,
    tick_id: i64,
    thought_log: Option<String>,
    action_type: String,
    action_data: Option<serde_json::Value>,
    priority: i32,
    subsequent_intents: Vec<Intent>,
    soul_cycle_metadata: Option<cyber_jianghu_protocol::SoulCycleMetadata>,
    chaos_marker: Option<cyber_jianghu_protocol::types::ChaosMarker>,
    dream_marker: Option<cyber_jianghu_protocol::types::DreamMarker>,
    state: &Arc<crate::state::AppState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 确定最终的 agent_id
    // 如果客户端指定了 agent_id，验证其属于该 device
    let agent_id = match msg_agent_id {
        Some(id) if id != uuid::Uuid::nil() => {
            // 使用 query_scalar 只查询 device_id，避免 SELECT *
            let owner_device_id: Option<uuid::Uuid> =
                sqlx::query_scalar("SELECT device_id FROM agents WHERE agent_id = $1")
                    .bind(id)
                    .fetch_optional(&state.db_pool)
                    .await
                    .context("查询 Agent 归属失败")?;

            match owner_device_id {
                Some(owner) if owner == device_id => {
                    tracing::debug!("Agent {} ownership verified for device {}", id, device_id);
                    id
                }
                Some(_) => {
                    tracing::warn!(
                        "Agent ownership mismatch: agent={}, device={}",
                        id,
                        device_id
                    );
                    return Err("无权操作此角色".into());
                }
                None => {
                    return Err("Agent 不存在".into());
                }
            }
        }
        // nil / None → 使用连接绑定的 agent_id（即时 intent 由 WebSocket 后台填充）
        _ => connection_agent_id,
    };

    let intent_id = req_intent_id.unwrap_or_else(uuid::Uuid::new_v4);

    // Handler 层拒绝时发送 ExecutionResult 给 agent（让 OutcomeMemory 记录失败）
    let reject_and_notify = |err_msg: String| async {
        let governance_code = ServerGovernanceMapper::map_from_error(&err_msg);
        let msg = cyber_jianghu_protocol::ServerMessage::ExecutionResult {
            tick_id,
            intent_id,
            success: false,
            error: Some(err_msg.clone()),
            state_change_summary: None,
            governance_code: Some(governance_code),
        };
        let _ = crate::tick::send_to_agent(
            agent_id,
            &msg,
            &state.connection_manager,
            &state.agent_to_device_map,
        )
        .await;
        Err::<(), Box<dyn std::error::Error + Send + Sync>>(err_msg.into())
    };

    // 速率限制检查
    if !crate::state::check_rate_limit(&state.rate_limiter, agent_id).await {
        warn!("Rate limit exceeded for agent {}", agent_id);
        return reject_and_notify(
            "Rate limit exceeded. Please wait before sending another intent.".into(),
        )
        .await;
    }

    // Agent 存活检查：从 DashMap 内存缓存读取（实时模式，不再查 DB）
    // 防御性：如果 DashMap 缺失 entry（auto-rebirth 后 WS 重连竞态），从 DB 回补
    let is_alive = state
        .agent_state_cache
        .get(&agent_id)
        .map(|r| r.value().is_alive)
        .unwrap_or_else(|| {
            // DashMap 缺失，尝试从 DB 加载（防御 auto-rebirth race condition）
            warn!("Agent {} not in DashMap, attempting DB fallback", agent_id);
            false
        });

    // 如果 DashMap 没有命中，尝试从 DB 防御性加载
    if !is_alive
        && !state.agent_state_cache.contains_key(&agent_id)
        && let Ok(db_state) = crate::db::get_latest_agent_state(&state.db_pool, agent_id).await
        && db_state.is_alive
    {
        let current_tick = state
            .current_accepting_tick_id
            .load(std::sync::atomic::Ordering::Acquire);
        // 保留 DB 原始 tick_id，让 decay 引擎根据差值补算衰减
        state.agent_state_cache.insert(agent_id, db_state.clone());
        info!(
            "DashMap miss → DB defensive load: agent {} (db_tick={}, current_tick={})",
            agent_id, db_state.tick_id, current_tick
        );
    }

    // 二次检查：防御性加载后重新读 DashMap
    let is_alive = if !is_alive {
        state
            .agent_state_cache
            .get(&agent_id)
            .map(|r| r.value().is_alive)
            .unwrap_or(false)
    } else {
        true
    };

    if !is_alive {
        warn!(
            "Intent rejected: agent {} is dead or not in cache",
            agent_id
        );
        return reject_and_notify(format!("Agent {} is dead or not in cache", agent_id)).await;
    }

    info!(
        "Intent received from agent {}: tick={}, action={}",
        agent_id, tick_id, action_type
    );

    // 快捷代理：吃/喝 → 用（允许 LLM 以更自然的语言表达消耗意图）
    // 归一化后再赋回 action_type，后续的 ActionRegistry 查询和 executor 分发
    // 都使用归一化后的动作类型。原始值已记入日志供调试。
    let action_type = match action_type.as_str() {
        "吃" | "喝" => "用".to_string(),
        other => other.to_string(),
    };

    // 递归归一化 subsequent_intents 中的动作类型
    let mut subsequent_intents = subsequent_intents;
    for sub in &mut subsequent_intents {
        let normalized = match sub.action_type.as_str() {
            "吃" | "喝" => "用".to_string(),
            x => x.to_string(),
        };
        sub.action_type = crate::models::ActionType::new(normalized);
    }

    // 解析动作类型（数据驱动：直接使用字符串）
    let action = crate::models::ActionType::new(&action_type);

    // 验证 subsequent_intents 安全性
    let max_subsequent = crate::game_data::registry()
        .map(|c| {
            c.get()
                .game_rules
                .data
                .intent_batch
                .as_ref()
                .map(|ib| ib.max_intents_per_tick)
                .unwrap_or(3)
        })
        .unwrap_or(3)
        .saturating_sub(1); // 减去 primary intent 自身

    if subsequent_intents.len() > max_subsequent {
        warn!(
            "Pipeline 过长: agent={} 有 {} 个 subsequent intents，上限 {}",
            agent_id,
            subsequent_intents.len(),
            max_subsequent
        );
        return reject_and_notify(format!("Pipeline 过长: 最多 {} 个后续动作", max_subsequent))
            .await;
    }

    // 递归拒绝：只允许单层 pipeline
    for (i, sub) in subsequent_intents.iter().enumerate() {
        if !sub.subsequent_intents.is_empty() {
            warn!(
                "嵌套 pipeline 拒绝: agent={} subsequent[{}] 含嵌套 intents",
                agent_id, i
            );
            return reject_and_notify(
                "不支持嵌套 pipeline，subsequent intents 不可再包含 subsequent".into(),
            )
            .await;
        }
    }

    // agent_id 一致性验证
    for (i, sub) in subsequent_intents.iter().enumerate() {
        if sub.agent_id != uuid::Uuid::nil() && sub.agent_id != agent_id {
            warn!(
                "agent_id 不一致: agent={} subsequent[{}] agent_id={}",
                agent_id, i, sub.agent_id
            );
            return reject_and_notify(format!("subsequent intent[{}] agent_id 不一致", i)).await;
        }
    }

    // 构造 Intent（chaos_marker/dream_marker 从 ClientMessage 透传，不再硬编码丢弃）
    let mut intent = Intent {
        intent_id: req_intent_id.unwrap_or_else(uuid::Uuid::new_v4),
        agent_id,
        tick_id,
        thought_log,
        action_type: action,
        action_data: action_data.clone(),
        priority,
        reflector_thought: None,
        chaos_marker,
        dream_marker,
        already_broadcast: false,
        session_id: None,
        subsequent_intents,
    };

    let transmission = ActionRegistry::get(action_type.as_str())
        .map(|c| c.transmission)
        .unwrap_or_default();

    // Broadcast: 公共频道广播给同 Location 的所有在线 Agent
    if transmission == Transmission::Broadcast
        && let Some(content_value) = action_data.as_ref().and_then(|d| d.get("content"))
        && let Some(content_str) = content_value.as_str()
    {
        let location = state
            .agent_state_cache
            .get(&agent_id)
            .map(|r| r.value().node_id.clone())
            .ok_or_else(|| anyhow::anyhow!("Agent {} 不在缓存中", agent_id))?;

        // 独立任务：广播，避免阻塞 intent 处理主流程
        let state_clone = state.clone();
        let content_owned = content_str.to_string();
        let agent_id_for_log = agent_id;
        let intent_id_for_log = intent.intent_id;
        tokio::spawn(async move {
            if let Err(e) = super::broadcast::broadcast_speak_to_location(
                agent_id_for_log,
                &location,
                &content_owned,
                tick_id,
                &state_clone,
            )
            .await
            {
                tracing::warn!(
                    "Failed to broadcast speak intent immediately: agent={}, intent={}, error={}",
                    agent_id_for_log,
                    intent_id_for_log,
                    e
                );
            } else {
                tracing::debug!(
                    "Speak intent broadcast immediately to location {} for agent {}",
                    location,
                    agent_id_for_log
                );
            }
        });

        // 标记已广播
        intent.already_broadcast = true;
    }

    // Session: 定向 + 创建 Dialogue Session
    if transmission == Transmission::Session
        && let Some(target_value) = action_data.as_ref().and_then(|d| d.get("target_agent_id"))
        && let Some(target_id_str) = target_value.as_str()
    {
        let candidates: Vec<uuid::Uuid> = state
            .agent_state_cache
            .iter()
            .map(|r| r.value().agent_id)
            .collect();
        let target_agent_id =
            match cyber_jianghu_protocol::resolve_agent_id(target_id_str, &candidates) {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!("无法解析 target_agent_id: {} ({})", target_id_str, e);
                    return reject_and_notify(format!("无效的 target_agent_id: {}", e)).await;
                }
            };

        match state
            .dialogue_manager
            .create_session(agent_id, target_agent_id)
            .await
        {
            Ok(response) => {
                if let DialogueResponse::RequestForwarded { session_id, .. } = response {
                    intent.session_id = Some(session_id.clone());
                    tracing::debug!(
                        "Whisper intent created Dialogue Session {} for agent {}",
                        session_id,
                        agent_id
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to create Dialogue Session for whisper: agent={}, target={}, error={}",
                    agent_id,
                    target_agent_id,
                    e
                );
            }
        }
    }

    // 路由到 IntentWorker（非阻塞 try_send，队列满时返回错误）
    // 提取 subsequent 信息（intent 即将被 move 进 WorkerMessage）
    type SubsequentSummary = (
        String,
        Option<serde_json::Value>,
        Option<cyber_jianghu_protocol::types::ChaosMarker>,
        Option<cyber_jianghu_protocol::types::DreamMarker>,
    );
    let subsequent_summaries: Vec<SubsequentSummary> = intent
        .subsequent_intents
        .iter()
        .map(|si| {
            (
                si.action_type.to_string(),
                si.action_data.clone(),
                si.chaos_marker.clone(),
                si.dream_marker.clone(),
            )
        })
        .collect();

    match state
        .worker_tx
        .try_send(crate::tick::WorkerMessage::Intent {
            intent: Box::new(intent),
        }) {
        Ok(()) => {
            info!(
                "Intent queued for real-time processing: agent={}, action={}, tick={}",
                agent_id, action_type, tick_id
            );

            // 三魂元数据就地写入（与 intent 同一条消息到达，消除独立 SoulCycleReport 的丢失风险）
            if let Some(ref metadata) = soul_cycle_metadata {
                let metadata_json =
                    serde_json::to_value(metadata).unwrap_or(serde_json::Value::Null);
                if let Err(e) = crate::db::update_soul_cycle_metadata(
                    &state.db_pool,
                    agent_id,
                    tick_id,
                    0, // pipe_seq=0：主 intent
                    &metadata_json,
                )
                .await
                {
                    warn!(
                        "三魂元数据写入失败(随intent): agent={}, tick={}, err={:#}",
                        agent_id, tick_id, e
                    );
                }

                // subsequent 占位（pipe_seq≥1）
                let world_time = metadata.world_time.clone();
                // 该 tick 的模型与本条 intent 的元数据同时到达，占位行直接复用，
                // 不再写死 None：否则 agent 后续的 SoulCycleReport 一旦丢失，
                // 这些行在经历日志里就永久显示"模型未上报"
                let tick_model_id = metadata.cycles.iter().find_map(|c| c.model_id.clone());
                for (idx, (act_type, act_data, chaos, dream)) in
                    subsequent_summaries.iter().enumerate()
                {
                    let pipe_seq = (idx + 1) as i32;
                    let placeholder = cyber_jianghu_protocol::SoulCycleMetadata {
                        world_time: world_time.clone(),
                        cycles: vec![cyber_jianghu_protocol::SoulCycleAttempt {
                            attempt: 0,
                            renhun: cyber_jianghu_protocol::RenhunReport {
                                narrative: Some("后续意图".to_string()),
                                thought_log: None,
                                earth_tool_calls: None,
                            },
                            tianhun: cyber_jianghu_protocol::TianhunReport {
                                result: Some("approved".to_string()),
                                layers: vec![
                                    cyber_jianghu_protocol::LayerReport {
                                        layer: "layer1".to_string(),
                                        passed: true,
                                        detail: None,
                                    },
                                    cyber_jianghu_protocol::LayerReport {
                                        layer: "layer2".to_string(),
                                        passed: true,
                                        detail: None,
                                    },
                                    cyber_jianghu_protocol::LayerReport {
                                        layer: "layer3".to_string(),
                                        passed: true,
                                        detail: None,
                                    },
                                ],
                                reason: None,
                                per_intent_layers: None,
                            },
                            final_intent: Some(cyber_jianghu_protocol::FinalIntentReport {
                                intent_id: None,
                                action_type: Some(act_type.clone()),
                                action_data: act_data.clone(),
                                pipeline_actions: None,
                                chaos_marker: chaos.clone(),
                                dream_marker: dream.clone(),
                            }),
                            model_id: tick_model_id.clone(),
                        }],
                        immediate_intents: vec![],
                    };
                    let ph_json =
                        serde_json::to_value(&placeholder).unwrap_or(serde_json::Value::Null);
                    if let Err(e) = crate::db::update_soul_cycle_metadata(
                        &state.db_pool,
                        agent_id,
                        tick_id,
                        pipe_seq,
                        &ph_json,
                    )
                    .await
                    {
                        warn!(
                            "后续意图占位写入失败: agent={}, tick={}, pipe_seq={}, err={:#}",
                            agent_id, tick_id, pipe_seq, e
                        );
                    }
                }
            }
        }
        Err(e) => {
            warn!(
                "Intent queue full or closed: agent={}, error={}",
                agent_id, e
            );
            return reject_and_notify("Intent queue full, server busy".into()).await;
        }
    }

    Ok(())
}
