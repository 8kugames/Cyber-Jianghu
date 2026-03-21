// ============================================================================
// 连接生命周期管理
// ============================================================================
//
// 处理 Agent 的连接、重连、主循环和关闭
// ============================================================================

use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::models::Intent;

/// 检查是否应该记录重试日志（日志采样策略）
///
/// 策略：
/// - 前 5 次：每次都记录
/// - 第 6 次后：仅当重试次数为完全平方数时记录（9, 16, 25, 36...）
fn should_log_retry(attempt: u32) -> bool {
    if attempt <= 5 {
        return true;
    }
    // 检查是否为完全平方数
    let sqrt = (attempt as f64).sqrt() as u32;
    sqrt * sqrt == attempt
}

impl super::Agent {
    /// 运行 Agent 主循环
    ///
    /// 持续接收世界状态，做出决策，发送意图
    pub async fn run(&mut self) -> Result<()> {
        // 初始连接：无限重试（带日志采样）
        let mut connect_attempt = 0u32;
        loop {
            connect_attempt += 1;
            match self.client.connect().await {
                Ok(()) => break,
                Err(e) => {
                    if should_log_retry(connect_attempt) {
                        warn!(
                            "连接游戏服务器失败 (尝试 {}): {}, 5秒后重试...",
                            connect_attempt, e
                        );
                    }
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                }
            }
        }
        info!("Agent '{}' connected to server", self.character_name());

        // 设置游戏规则更新回调
        let agent_name_for_callback = self.character_name().to_string();
        self.client
            .set_game_rules_callback(Arc::new(move |game_rules| {
                info!(
                    "Agent '{}' received game rules update: version {}",
                    agent_name_for_callback, game_rules.version
                );
                // 注意：配置持久化由外部配置管理系统处理
            }));

        // 设置对话消息回调（如果启用了对话系统）
        if self.dialogue_client.is_some() {
            let dialogue_client = self.dialogue_client.clone();
            let agent_name_for_dialogue = self.character_name().to_string();
            self.client.set_dialogue_callback(Arc::new(move |message| {
                debug!(
                    "Agent '{}' received dialogue message",
                    agent_name_for_dialogue
                );
                if let Some(ref dc) = dialogue_client {
                    dc.handle_message(message);
                }
            }));
            info!(
                "Dialogue callback set for agent '{}'",
                self.character_name()
            );
        }

        // 设置世界观规则更新回调（如果启用了验证器）
        if self.validator.is_some() {
            let validator = self.validator.clone();
            let agent_name_for_rules = self.character_name().to_string();
            self.client
                .set_world_building_rules_callback(Arc::new(move |rules| {
                    info!(
                        "Agent '{}' received world building rules update: version {}",
                        agent_name_for_rules, rules.version
                    );
                    if let Some(ref v) = validator {
                        // 使用 tokio::spawn 因为回调不是 async
                        let v_clone = v.clone();
                        let rules_clone = rules.clone();
                        tokio::spawn(async move {
                            v_clone.update_rules(rules_clone).await;
                        });
                    }
                }));
            info!(
                "World building rules callback set for agent '{}'",
                self.character_name()
            );
        }

        // 等待注册确认（包含游戏规则）
        let (agent_id, game_rules) = self.client.wait_for_registration().await?;
        info!("Agent '{}' registered with server", self.character_name());
        info!("Server-assigned Agent ID: {}", agent_id);

        // 调用注册回调（更新外部状态如 HTTP API 的 agent_id）
        if let Some(ref callback) = self.registration_callback {
            callback(agent_id);
        }
        info!(
            "Received game rules: version {}, {} actions, {} initial items",
            game_rules.version,
            game_rules.available_actions.len(),
            game_rules.initial_items.len()
        );

        // 更新游戏规则
        self.config.update_game_rules(game_rules.clone());

        loop {
            // 1. 接收世界状态
            let world_state = match self.client.receive_world_state().await {
                Ok(state) => state,
                Err(e) => {
                    error!("Failed to receive world state: {}", e);
                    // 尝试重连
                    self.reconnect().await?;
                    continue;
                }
            };

            // 1.5 检查是否死亡
            if let Some(death_event) = world_state.events_log.iter().find(|e| {
                if let Some(cause) = e.metadata.get("cause")
                    && let Some(cause_str) = cause.as_str() {
                        return cause_str.starts_with("death");
                    }
                if let Some(msg_type) = e.metadata.get("type")
                    && let Some(type_str) = msg_type.as_str() {
                        return type_str == "death_notification";
                    }
                false
            }) {
                warn!(
                    "Agent '{}' has died: {}",
                    self.character_name(), death_event.description
                );
                // 可以在这里处理死亡逻辑，例如退出循环或进入观察者模式
                // 目前 MVP 阶段，我们只是记录日志并继续（可能会尝试发送 idle 直到被踢出）
            }

            // 2. 处理事件并更新记忆
            if let Err(e) = self.process_events(&world_state.events_log).await {
                warn!("Failed to process events into memory: {}", e);
            }

            // 3. 每 84 tick 运行遗忘机制
            if world_state.tick_id % 84 == 0
                && let Err(e) = self.run_forgetting(world_state.tick_id).await {
                    warn!("Failed to run forgetting mechanism: {}", e);
                }

            // 4. 构建增强的世界状态（包含记忆上下文）
            let memory_context = self.get_memory_context().await;
            if !memory_context.is_empty() {
                debug!("Memory context:\n{}", memory_context);
            }

            // 5. 调用决策回调（带验证和记忆上下文）
            let intent = if self.validator.is_some() {
                self.decide_with_validation(&world_state).await?
            } else if let Some(ref memory_callback) = self.decision_with_memory_callback {
                // 优先使用带记忆上下文的回调
                memory_callback(&world_state, &memory_context).await
            } else {
                (self.decision_callback)(&world_state).await
            };

            // 6. 更新寿命状态（如果启用）
            if let Some(ref mut calculator) = self.lifespan_calculator {
                let status = calculator.process_tick();
                if status.is_deceased() {
                    info!(
                        "Agent '{}' has passed away at age {}",
                        self.character_name(),
                        status.age()
                    );
                    // 发送最后一个 idle 意图后退出
                    self.client
                        .send_intent(&Intent::idle(
                            self.client.agent_id().unwrap(),
                            world_state.tick_id,
                        ))
                        .await
                        .ok();
                    return Ok(());
                }
            }

            // 7. 发送意图
            if let Err(e) = self.client.send_intent(&intent).await {
                error!("Failed to send intent: {}", e);
                // 尝试重连
                self.reconnect().await?;
            }
        }
    }

    /// 重连服务端（指数退避策略）
    async fn reconnect(&mut self) -> Result<()> {
        const MAX_RETRIES: u32 = 5;
        const INITIAL_DELAY_MS: u64 = 1000; // 1 秒
        const MAX_DELAY_MS: u64 = 30000; // 30 秒

        self.client.close().await;

        for attempt in 1..=MAX_RETRIES {
            // 计算指数退避延迟
            let delay_ms = std::cmp::min(INITIAL_DELAY_MS * (1 << (attempt - 1)), MAX_DELAY_MS);

            warn!(
                "Reconnection attempt {}/{} (waiting {}ms)...",
                attempt, MAX_RETRIES, delay_ms
            );

            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;

            match self.client.connect().await {
                Ok(()) => {
                    info!("Reconnected successfully after {} attempts", attempt);
                    return Ok(());
                }
                Err(e) if attempt < MAX_RETRIES => {
                    warn!("Reconnection attempt {} failed: {}", attempt, e);
                }
                Err(e) => {
                    error!("All reconnection attempts failed: {}", e);
                    return Err(anyhow::anyhow!("All reconnection attempts failed: {}", e));
                }
            }
        }

        Err(anyhow::anyhow!(
            "Failed to reconnect after {} attempts",
            MAX_RETRIES
        ))
    }

    /// 关闭连接
    pub async fn close(&mut self) -> Result<()> {
        self.client.close().await;
        info!("Agent '{}' stopped", self.character_name());
        Ok(())
    }
}
