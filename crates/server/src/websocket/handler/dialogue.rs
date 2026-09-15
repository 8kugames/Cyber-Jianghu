// ============================================================================
// 对话消息处理与转发
// ============================================================================

use super::*;

/// 处理对话消息
///
pub(super) async fn handle_dialogue_message(
    agent_id: uuid::Uuid,
    message: DialogueMessage,
    state: &Arc<crate::state::AppState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("对话消息来自 agent {}: {:?}", agent_id, message);

    // 调用对话管理器处理消息
    let response = state.dialogue_manager.handle_message(message.clone()).await;

    match response {
        Ok(dialogue_response) => {
            match dialogue_response {
                DialogueResponse::RequestForwarded {
                    session_id,
                    target_agent_id,
                } => {
                    // 转发请求给目标 Agent
                    let forward_msg = DialogueMessage::Request {
                        from_agent_id: agent_id,
                        to_agent_id: target_agent_id,
                        opening_remark: match &message {
                            DialogueMessage::Request { opening_remark, .. } => {
                                opening_remark.clone()
                            }
                            _ => String::new(),
                        },
                    };
                    super::super::broadcast::forward_dialogue_message(
                        target_agent_id,
                        forward_msg,
                        &state.connection_manager,
                        &state.agent_to_device_map,
                    )
                    .await?;
                    debug!(
                        "对话请求已转发: session={}, to={}",
                        session_id, target_agent_id
                    );
                }
                DialogueResponse::SessionStarted {
                    session_id,
                    agent_a,
                    agent_b,
                } => {
                    // 通知双方会话已建立
                    let started_msg = DialogueMessage::Accept {
                        session_id: session_id.clone(),
                        from_agent_id: agent_id,
                    };
                    super::super::broadcast::forward_dialogue_message(
                        agent_a,
                        started_msg.clone(),
                        &state.connection_manager,
                        &state.agent_to_device_map,
                    )
                    .await?;
                    super::super::broadcast::forward_dialogue_message(
                        agent_b,
                        started_msg,
                        &state.connection_manager,
                        &state.agent_to_device_map,
                    )
                    .await?;
                    debug!("会话已建立，双方已通知: session={}", session_id);
                }
                DialogueResponse::SessionRejected {
                    session_id,
                    rejected_by,
                    requester,
                } => {
                    // 通知请求发起者被拒绝
                    let rejected_msg = DialogueMessage::Reject {
                        session_id: session_id.clone(),
                        from_agent_id: rejected_by,
                        reason: None,
                    };
                    super::super::broadcast::forward_dialogue_message(
                        requester,
                        rejected_msg,
                        &state.connection_manager,
                        &state.agent_to_device_map,
                    )
                    .await?;
                    debug!(
                        "会话已拒绝: session={}, rejected_by={}, notified={}",
                        session_id, rejected_by, requester
                    );
                }
                DialogueResponse::ContentForward {
                    session_id,
                    from_agent_id,
                    to_agent_id,
                } => {
                    // 转发内容给目标 Agent
                    let content_msg = match &message {
                        DialogueMessage::Content {
                            session_id,
                            from_agent_id,
                            content,
                        } => DialogueMessage::Content {
                            session_id: session_id.clone(),
                            from_agent_id: *from_agent_id,
                            content: content.clone(),
                        },
                        _ => return Err("Invalid message type for ContentForward".into()),
                    };
                    super::super::broadcast::forward_dialogue_message(
                        to_agent_id,
                        content_msg,
                        &state.connection_manager,
                        &state.agent_to_device_map,
                    )
                    .await?;
                    debug!(
                        "对话内容已转发: session={}, from={}, to={}",
                        session_id, from_agent_id, to_agent_id
                    );
                }
                DialogueResponse::SessionEnded {
                    session_id,
                    ended_by,
                    other_participant,
                } => {
                    // 通知另一方会话已结束
                    let end_msg = DialogueMessage::End {
                        session_id: session_id.clone(),
                        from_agent_id: ended_by,
                    };
                    super::super::broadcast::forward_dialogue_message(
                        other_participant,
                        end_msg,
                        &state.connection_manager,
                        &state.agent_to_device_map,
                    )
                    .await?;
                    debug!(
                        "会话已结束: session={}, ended_by={}, notified={}",
                        session_id, ended_by, other_participant
                    );
                }
            }
        }
        Err(e) => {
            warn!("对话消息处理失败: {}", e);
            // 发送错误消息给发起者（通过 agent_to_device_map 解析 device_id）
            let error_msg = ServerMessage::Error {
                code: cyber_jianghu_protocol::ERROR_CODE_DIALOGUE_FAILED.to_string(),
                message: format!("Dialogue failed: {}", e),
                current_tick_id: None,
            };
            let json = serde_json::to_string(&error_msg)?;
            let device_id = {
                let agent_to_device = state.agent_to_device_map.read().await;
                agent_to_device.get(&agent_id).copied()
            };
            if let Some(device_id) = device_id {
                let connections = state.connection_manager.read().await;
                if let Some(connection) = connections.get(&device_id)
                    && let Err(e) = connection.send(Message::Text(json.into())).await
                {
                    tracing::warn!(
                        "ws connection.send（broadcast）失败（receiver 可能已 drop）：{e:?}"
                    );
                }
            }
        }
    }

    Ok(())
}
