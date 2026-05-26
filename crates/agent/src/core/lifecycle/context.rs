// ============================================================================
// 决策上下文构建
// ============================================================================
//
// 从 WorldState 构建完整的记忆上下文：
//   - 消费 rejection 反馈 + 即时事件
//   - 处理世界事件 + 社交事件 + 遗忘
//   - 拼接交易提示 + triage 事件
//
// 调用路径: run() → build_tick_memory_context() → (memory_context, trade_hints)
// ============================================================================

use cyber_jianghu_protocol::WorldState;
use tracing::{debug, warn};

impl super::super::Agent {
    /// 构建 tick 决策所需的记忆上下文
    ///
    /// 处理流程:
    /// 1. 清除上轮 rejection + 消费 Server 错误反馈
    /// 2. 消费即时事件缓冲区 → 工作记忆
    /// 3. 处理 WorldState.events_log → 叙事合成记忆
    /// 4. 社交事件 → 关系更新
    /// 5. 遗忘机制
    /// 6. 基础 memory_context + 对话上下文
    /// 7. 交易议价提示
    /// 8. Triage 事件注入
    pub(crate) async fn build_tick_memory_context(
        &mut self,
        world_state: &WorldState,
    ) -> (String, Vec<String>) {
        // 1. 清除上一 tick 的 rejection reason（在消费新反馈之前）
        self.last_rejection_reason = None;

        // 1.6 消费 Server 验证错误反馈（由 Fn callback 异步写入）
        {
            let mut guard = self.server_error_feedback.lock().await;
            if let Some(reason) = guard.take() {
                warn!("Server 验证错误反馈: {}", reason);
                self.last_rejection_reason =
                    Some(super::super::Agent::narrativize_rejection(&reason));
            }
        }

        // 1.7 消费即时事件缓冲区（ImmediateEvent 即时写入工作记忆）
        let immediate_events = {
            let mut guard = self.immediate_event_buffer.lock().await;
            if guard.is_empty() {
                Vec::new()
            } else {
                guard.drain(..).collect()
            }
        };
        if !immediate_events.is_empty() {
            debug!("消费 {} 个即时事件到工作记忆", immediate_events.len());
            // 即时事件不经过叙事合成（直接工作记忆）
            if let Err(e) = self.process_events(&immediate_events, None).await {
                warn!("即时事件写入记忆失败: {}", e);
            }
        }

        // 2. 处理事件并更新记忆（叙事合成）
        // 先 clone Arc 让 borrow 立即结束，避免与后续的 &mut self 冲突
        let cognitive_engine_ref = self.cognitive_engine.as_ref().cloned();
        if let Err(e) = self
            .process_events(&world_state.events_log, cognitive_engine_ref.as_deref())
            .await
        {
            warn!("Failed to process events into memory: {}", e);
        }

        // 2.5 社交事件 → 自动更新关系（非阻塞，spawn 后台任务）
        self.process_social_events(&world_state.events_log, &world_state.entities);

        // 3. 遗忘机制（间隔由 memory.forgetting_interval_ticks 配置）
        if world_state.tick_id % self.config.memory.forgetting_interval_ticks == 0
            && let Err(e) = self.run_forgetting(world_state.tick_id).await
        {
            warn!("Failed to run forgetting mechanism: {}", e);
        }

        // 4. 构建增强的世界状态（包含记忆上下文）
        let mut memory_context = self.get_memory_context().await;

        // 4.1 交易议价提示（经济引导，非生存干预）
        // 附近有其他人且有银两时注入交易建议（关系感知）
        let trade_hints = {
            let mut hints = Vec::new();
            let has_silver = world_state
                .self_state
                .inventory
                .iter()
                .any(|i| i.item_id == "银子" && i.quantity > 0);
            if !world_state.entities.is_empty() && has_silver {
                let silver = world_state
                    .self_state
                    .inventory
                    .iter()
                    .find(|i| i.item_id == "银子")
                    .map(|i| i.quantity)
                    .unwrap_or(0);

                let mut entity_descs = Vec::new();
                for entity in &world_state.entities {
                    let rel_desc = self
                        .relationship_store
                        .as_ref()
                        .and_then(|store| store.get_relationship(entity.id).ok().flatten())
                        .map(|rel| {
                            let (_, label) =
                                crate::component::social::get_relationship_level(rel.favorability);
                            format!("{}（{}，好感度{}）", entity.name, label, rel.favorability)
                        })
                        .unwrap_or_else(|| format!("{}（陌生人，好感度0）", entity.name));
                    entity_descs.push(rel_desc);
                }

                hints.push(format!(
                    "【交易提示】你可以使用「说话」与对方讨价还价。先询价，协商好价格后再用「给予」交付物品换取银两。关系越好价格越优惠。你身边有：{}。你身上有{}两银子。",
                    entity_descs.join("、"),
                    silver,
                ));
            }
            hints
        };

        // 4.2 Sanity：已移除天道干预式警告注入
        // 低理智行为由 chaos generator (chaos.rs) 处理，体感叙事由
        // WorldState.attribute_descriptions 提供（sanity 阈值 narrative_config.yaml）
        if let Some(ref handler) = self.immediate_handler {
            let store = handler.event_store();
            let config = store.config();
            let game_day = Self::compute_game_day(
                &world_state.world_time,
                self.config
                    .game_rules
                    .as_ref()
                    .and_then(|g| g.calendar.as_ref()),
            );
            match store
                .query_triaged_async(config.context.clone(), game_day)
                .await
            {
                Ok(triaged) => {
                    // URGENT: 逐条高可见性展示
                    for event in &triaged.urgent {
                        let sender = event.from_agent_name.as_deref().unwrap_or("有人");
                        memory_context.push_str(&format!(
                            "\n!! 紧急事件: {}「{}」",
                            sender, event.description
                        ));
                    }

                    // BATCH: 摘要格式展示
                    if !triaged.batch.is_empty() {
                        let batch_lines: Vec<String> = triaged
                            .batch
                            .iter()
                            .take(config.context.max_batch_summary_chars / 20) // 粗略条目数限制
                            .map(|e| {
                                let sender = e.from_agent_name.as_deref().unwrap_or("有人");
                                format!("- {}: {}", sender, e.description)
                            })
                            .collect();
                        let batch_summary = batch_lines.join("\n");
                        if !batch_summary.is_empty() {
                            memory_context
                                .push_str(&format!("\n### 近期事件摘要\n{}\n", batch_summary));
                        }
                    }

                    // 标记已消费（按 ID，避免与后台 triage 竞态）
                    if !triaged.urgent.is_empty() || !triaged.batch.is_empty() {
                        let consumed_ids: Vec<i64> = triaged
                            .urgent
                            .iter()
                            .chain(triaged.batch.iter())
                            .map(|e| e.id)
                            .collect();
                        if let Err(e) = store
                            .mark_processed_by_ids_async(consumed_ids, world_state.tick_id)
                            .await
                        {
                            warn!("标记已消费事件失败: {}", e);
                        }
                    }
                }
                Err(e) => {
                    warn!("查询 triage 事件失败: {}", e);
                }
            }
        }

        if !memory_context.is_empty() {
            debug!("Memory context:\n{}", memory_context);
        }

        (memory_context, trade_hints)
    }
}
