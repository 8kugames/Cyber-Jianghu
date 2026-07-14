use tracing::{debug, error, info, warn};

impl super::super::Agent {
    /// 从 SoulCycleRecorder 构建本 tick 的三魂循环元数据
    ///
    /// 三魂审查在 intent 提交前完成，recorder 数据已就绪。
    /// 此方法在 send_intent 之前调用，metadata 随 intent 一次性提交。
    pub(super) async fn build_soul_cycle_metadata(
        &self,
        tick_id: i64,
    ) -> Option<cyber_jianghu_protocol::SoulCycleMetadata> {
        let recorder = self.soul_recorder().await?;

        let records = match recorder.get_by_tick(tick_id).await {
            Ok(r) => r,
            Err(e) => {
                warn!("build_soul_cycle_metadata: get_by_tick({tick_id}) 失败: {e:?}");
                return None;
            }
        };
        if records.is_empty() {
            return None;
        }

        let immediate_records = recorder.get_immediate_by_tick(tick_id).await.unwrap_or_default();

        let world_time = records.first().and_then(|r| r.world_time.clone());

        let cycles: Vec<cyber_jianghu_protocol::SoulCycleAttempt> = records
            .into_iter()
            .map(|r| {
                let layers: Vec<cyber_jianghu_protocol::LayerReport> = vec![
                    (r.tianhun_layer1_result.as_deref(), "layer1"),
                    (r.tianhun_layer2_result.as_deref(), "layer2"),
                    (r.tianhun_layer3_result.as_deref(), "layer3"),
                ]
                .into_iter()
                .filter_map(|(detail, layer)| {
                    detail.map(|d| cyber_jianghu_protocol::LayerReport {
                        layer: layer.to_string(),
                        passed: d == "通过" || d.is_empty(),
                        detail: if d == "通过" || d.is_empty() {
                            None
                        } else {
                            Some(d.to_string())
                        },
                    })
                })
                .collect();

                cyber_jianghu_protocol::SoulCycleAttempt {
                    attempt: r.attempt,
                    renhun: cyber_jianghu_protocol::RenhunReport {
                        narrative: r.renhun_narrative,
                        thought_log: r.renhun_thought_log,
                        earth_tool_calls: r
                            .earth_tool_calls
                            .as_ref()
                            .and_then(|s| serde_json::from_str(s).ok()),
                    },
                    tianhun: cyber_jianghu_protocol::TianhunReport {
                        result: r.tianhun_result,
                        layers,
                        reason: r.tianhun_reason,
                    },
                    final_intent: r.final_intent_id.map(|id| {
                        let pipeline_actions: Option<
                            Vec<cyber_jianghu_protocol::PipelineAction>,
                        > = r
                            .final_pipeline_json
                            .as_ref()
                            .and_then(|s| serde_json::from_str(s).ok());
                        cyber_jianghu_protocol::FinalIntentReport {
                            intent_id: Some(id),
                            action_type: r.final_action_type.clone(),
                            action_data: r
                                .final_action_data
                                .as_ref()
                                .and_then(|s| serde_json::from_str(s).ok()),
                            pipeline_actions,
                            chaos_marker: None,
                            dream_marker: None,
                        }
                    }),
                    model_id: r.model_id,
                }
            })
            .collect();

        let agent_name = self.character_name().to_string();
        let immediate_intents: Vec<cyber_jianghu_protocol::ImmediateIntentReport> =
            immediate_records
                .into_iter()
                .map(|r| cyber_jianghu_protocol::ImmediateIntentReport {
                    intent_id: r.intent_id,
                    route_type: r.route_type,
                    action_type: r.action_type,
                    action_data: r
                        .action_data
                        .as_ref()
                        .and_then(|s| serde_json::from_str(s).ok()),
                    from_agent_name: Some(agent_name.clone()),
                    speech_content: r.speech_content,
                    send_status: r.send_status,
                    send_error: r.send_error,
                })
                .collect();

        Some(cyber_jianghu_protocol::SoulCycleMetadata {
            world_time,
            cycles,
            immediate_intents,
        })
    }

    pub(super) async fn report_soul_cycle_and_compress(
        &self,
        final_intent: &crate::models::Intent,
    ) {
        let tick_id_for_report = final_intent.tick_id;
        if self.soul_recorder().await.is_some() {
            // 主 metadata 已随 intent 提交（build_soul_cycle_metadata + send_intent），
            // 此处不再单独发送 SoulCycleReport（消除独立消息的丢失风险）。
            // 保留 subsequent intents 的 SoulCycleReport 发送（过渡期）+ 对话 summary 压缩。

            // 主 metadata 已随 intent 提交，此处仅发送 subsequent 的简化占位（过渡期保留）
            let subsequent_count = final_intent.subsequent_intents.len();
            let max_retries = self.config.llm.soul_cycle_report_retries;
            let base_delay = self.config.llm.soul_cycle_report_base_delay_ms;
            if subsequent_count > 0 {
                let metadata = self.build_soul_cycle_metadata(tick_id_for_report).await;
                let world_time = metadata
                    .as_ref()
                    .and_then(|m| m.world_time.clone());
                for (idx, subsequent) in final_intent.subsequent_intents.iter().enumerate() {
                    let pipe_seq = (idx + 1) as i32;
                    let simplified_metadata = cyber_jianghu_protocol::SoulCycleMetadata {
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
                            },
                            final_intent: Some(cyber_jianghu_protocol::FinalIntentReport {
                                intent_id: Some(subsequent.intent_id.to_string()),
                                action_type: Some(subsequent.action_type.to_string()),
                                action_data: subsequent.action_data.clone(),
                                pipeline_actions: None,
                                chaos_marker: subsequent.chaos_marker.clone(),
                                dream_marker: subsequent.dream_marker.clone(),
                            }),
                            model_id: None,
                        }],
                        immediate_intents: vec![],
                    };

                    let mut reported = false;
                    for attempt in 0..max_retries {
                        match self
                            .client
                            .send_soul_cycle_report(
                                tick_id_for_report,
                                pipe_seq,
                                simplified_metadata.clone(),
                            )
                            .await
                        {
                            Ok(()) => {
                                debug!(
                                    "后续意图元数据上报成功: tick={}, pipe_seq={}, action={}",
                                    tick_id_for_report, pipe_seq, subsequent.action_type
                                );
                                reported = true;
                                break;
                            }
                            Err(e) => {
                                warn!(
                                    "后续意图元数据上报失败 (尝试 {}/{}): tick={}, pipe_seq={}, err={}",
                                    attempt + 1,
                                    max_retries,
                                    tick_id_for_report,
                                    pipe_seq,
                                    e
                                );
                                if attempt + 1 < max_retries {
                                    tokio::time::sleep(tokio::time::Duration::from_millis(
                                        base_delay * (1 << attempt),
                                    ))
                                    .await;
                                }
                            }
                        }
                    }
                    if !reported {
                        error!(
                            "后续意图元数据上报最终失败: tick={}, pipe_seq={}",
                            tick_id_for_report, pipe_seq
                        );
                    }
                }
            }
        }

        if let Some(ref engine) = self.cognitive_engine
            && engine.conversation_needs_summary()
            && let Some(prompt) = engine.conversation_summary_prompt()
            && let Some(ref container) = self.actor_llm_container
        {
            let llm = container.read().await;
            if let Ok(summary) = llm.complete(&prompt).await {
                engine.conversation_replace_with_summary(summary);
                info!("对话历史 summary 压缩完成");
            } else {
                tracing::warn!("对话历史 summary 生成失败，降级为强制截断");
                engine.conversation_force_truncate();
            }
        }
    }
}
