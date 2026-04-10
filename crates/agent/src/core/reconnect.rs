// ============================================================================
// 重连与转生逻辑
// ============================================================================
//
// 处理 Agent 的重连、token 刷新、等待转生和角色配置持久化
// ============================================================================

use anyhow::Result;
use cyber_jianghu_protocol::ServerMessage;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::config::{CharacterConfig, CharacterStatus};
use crate::infra::transport::ConnectError;

/// 检查是否应该记录重试日志（日志采样策略）
///
/// 策略：
/// - 前 5 次：每次都记录
/// - 第 6 次后：仅当重试次数为完全平方数时记录（9, 16, 25, 36...）
pub(crate) fn should_log_retry(attempt: u32) -> bool {
    if attempt <= 5 {
        return true;
    }
    let sqrt = (attempt as f64).sqrt() as u32;
    sqrt * sqrt == attempt
}

impl super::Agent {
    /// 重连服务端（无限重试，逐步降频策略）
    ///
    /// 降频策略：
    /// - 初始延迟 1 秒
    /// - 每次失败后延迟翻倍
    /// - 最大延迟为 tick_duration 的一半（确保每个 tick 至少尝试 2 次）
    /// - 重连成功后重置退避计数器
    pub(crate) async fn reconnect(&mut self) -> Result<()> {
        const INITIAL_DELAY_MS: u64 = 1000; // 1 秒

        // 获取 tick 时长，计算最大延迟（tick 的一半）
        let tick_duration_ms = self.get_tick_duration().await.as_millis() as u64;
        let max_delay_ms = tick_duration_ms / 2;

        self.client.close().await;

        loop {
            // 计算当前延迟：初始延迟 * 2^backoff，但不超过最大延迟
            let delay_ms = std::cmp::min(
                INITIAL_DELAY_MS * (1u64 << self.reconnect_backoff.min(10)),
                max_delay_ms,
            );

            let attempt = self.reconnect_backoff + 1;
            if should_log_retry(attempt) {
                warn!(
                    "重连尝试 {} (等待 {}ms, 最大 {}ms)...",
                    attempt, delay_ms, max_delay_ms
                );
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;

            match self.client.connect().await {
                Ok(()) => {
                    info!("重连成功，尝试次数: {}", attempt);

                    // 等待 Server 发送 Registered 消息，获取最新的 agent_id 和 game_rules
                    match self.client.wait_for_registration().await {
                        Ok(Some((agent_id, game_rules, registered_name, is_alive))) => {
                            info!("重连后注册确认: agent_id={}, alive={}", agent_id, is_alive);

                            // 更新 agent 名称
                            if let Some(ref name) = registered_name {
                                self.server_assigned_name = Some(name.clone());
                                if let Some(ref engine) = self.cognitive_engine {
                                    engine.update_agent_name(name);
                                }
                                info!("从服务器获取角色名称: {}", name);
                            }

                            // agent_id 为零 = 角色已归隐（可能在等待期间被删除）
                            if agent_id == Uuid::nil() {
                                warn!("重连后收到 nil agent_id，角色已归隐");
                                self.death_reported = true;
                                if let Some(ref api_state) = self.http_api_state {
                                    api_state
                                        .is_dead
                                        .store(true, std::sync::atomic::Ordering::Relaxed);
                                    let death_msg = ServerMessage::AgentDied {
                                        agent_id: Uuid::nil(),
                                        cause: "retired".to_string(),
                                        description: "角色已归隐，请创建新角色".to_string(),
                                        location: String::new(),
                                        tick_id: 0,
                                        died_at: chrono::Utc::now().timestamp_millis(),
                                        rebirth_delay_ticks: 0,
                                    };
                                    let _ = api_state.death_event_tx.send(death_msg);
                                }
                                // 归隐后不返回错误，保持进程存活等待创建新角色
                                return Ok(());
                            }

                            // 服务器返回真实 agent_id 但 is_alive=false：断连期间角色死亡
                            if !is_alive {
                                warn!("重连后发现角色 {} 已死亡 (is_alive=false)", agent_id);
                                self.death_reported = true;
                                if let Some(ref api_state) = self.http_api_state {
                                    api_state
                                        .is_dead
                                        .store(true, std::sync::atomic::Ordering::Relaxed);
                                    let death_msg = ServerMessage::AgentDied {
                                        agent_id,
                                        cause: "disconnect_death".to_string(),
                                        description:
                                            "角色在断连期间死亡，请通过 rebirth 创建新角色"
                                                .to_string(),
                                        location: String::new(),
                                        tick_id: 0,
                                        died_at: chrono::Utc::now().timestamp_millis(),
                                        rebirth_delay_ticks: 0,
                                    };
                                    let _ = api_state.death_event_tx.send(death_msg);
                                }

                                // 持久化死亡状态
                                if let Some(ref mut char_cfg) = self.character_config {
                                    char_cfg.status = CharacterStatus::Dead;
                                    if let Some(ref api_state) = self.http_api_state {
                                        let characters_dir =
                                            api_state.character_dir.read().await.clone();
                                        if let Err(e) =
                                            save_character_config_to_fs(char_cfg, &characters_dir)
                                        {
                                            warn!(
                                                "Failed to persist reconnect-death status: {}",
                                                e
                                            );
                                        }
                                    }
                                }

                                // 保持进程存活等待 rebirth
                                return Ok(());
                            }

                            // 重置死亡状态（转生后获得新身份）
                            self.death_reported = false;
                            if let Some(ref api_state) = self.http_api_state {
                                api_state
                                    .is_dead
                                    .store(false, std::sync::atomic::Ordering::Relaxed);
                            }

                            // 自动重建本地 character.yaml（reconnect 路径）
                            if self.character_config.is_none() {
                                let s_dir = self.config.server_dir(&self.config.server.ws_url);
                                let chars_dir = s_dir.join("characters");
                                let c_dir = chars_dir.join(agent_id.to_string());
                                let c_yaml = c_dir.join("character.yaml");

                                if !c_yaml.exists() {
                                    let name = registered_name.as_deref().unwrap_or("未知");
                                    let recon = CharacterConfig {
                                        agent_id: Some(agent_id),
                                        name: name.to_string(),
                                        status: CharacterStatus::Alive,
                                        server_url: Some(self.config.server.http_url.clone()),
                                        registered_at: Some(chrono::Utc::now()),
                                        ..Default::default()
                                    };

                                    if let Err(e) = (|| -> anyhow::Result<()> {
                                        std::fs::create_dir_all(&c_dir)?;
                                        recon.save_to_file(&c_yaml)?;
                                        Ok(())
                                    })() {
                                        warn!("reconnect 自动重建 character.yaml 失败: {}", e);
                                    } else {
                                        info!(
                                            "reconnect 已自动重建角色配置: {} ({})",
                                            name, agent_id
                                        );
                                        self.character_config = Some(recon);
                                    }
                                }
                            }

                            // 调用注册回调（更新外部状态如 HTTP API 的 agent_id）
                            if let Some(ref callback) = self.registration_callback {
                                callback(agent_id);
                            }

                            // 更新游戏规则
                            self.config.update_game_rules(game_rules);
                        }
                        Ok(None) => {
                            // agent_id 为 nil，等待角色注册，保持连接
                            info!("重连后收到 nil agent_id，等待角色注册...");
                            self.death_reported = true;
                            if let Some(ref api_state) = self.http_api_state {
                                api_state
                                    .is_dead
                                    .store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            // 不返回错误，让调用方知道需要等待
                            return Ok(());
                        }
                        Err(e) => {
                            // 其他错误，继续重试
                            tracing::error!("重连后注册确认失败: {}", e);
                            self.client.close().await;
                            // 增加退避计数器并继续重试
                            self.reconnect_backoff = self.reconnect_backoff.saturating_add(1);
                            continue;
                        }
                    }

                    // 重连成功，重置退避计数器
                    self.reconnect_backoff = 0;
                    return Ok(());
                }
                Err(ConnectError::AuthFailed) => {
                    warn!(
                        "重连 auth failed (attempt {}), refreshing token...",
                        attempt
                    );
                    match self.refresh_device_token().await {
                        Ok(()) => {
                            info!("Token refreshed, retrying reconnection...");
                            // 不增加退避计数器，因为 token 已刷新
                            continue;
                        }
                        Err(e) => {
                            if should_log_retry(attempt) {
                                warn!("重连 token refresh 失败 (attempt {}): {}", attempt, e);
                            }
                            // 增加退避计数器（逐步降低频率）
                            self.reconnect_backoff = self.reconnect_backoff.saturating_add(1);
                            // 继续循环，不退出
                        }
                    }
                }
                Err(ConnectError::ConnectionFailed(e)) => {
                    if should_log_retry(attempt) {
                        warn!("重连尝试 {} 失败: {}", attempt, e);
                    }
                    // 增加退避计数器（逐步降低频率）
                    self.reconnect_backoff = self.reconnect_backoff.saturating_add(1);
                    // 继续循环，不退出
                }
            }
        }
    }

    /// 刷新设备 token（WebSocket 400 认证失败时自动调用）
    ///
    /// 调用 `POST {server_http_url}/api/v1/agent/connect` 获取新的 auth_token，
    /// 然后更新客户端身份和本地 device_config。
    pub(crate) async fn refresh_device_token(&mut self) -> Result<()> {
        let device_id = self
            .device_config
            .as_ref()
            .map(|d| d.device_id)
            .ok_or_else(|| anyhow::anyhow!("No device_config, cannot refresh token"))?;

        let http_url = &self.config.server.http_url;
        let url = format!("{}/api/v1/agent/connect", http_url);

        debug!("Refreshing device token for {} at {}", device_id, url);

        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .json(&serde_json::json!({ "device_id": device_id }))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("HTTP request failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Server returned error {}: {}", status, body);
        }

        #[derive(serde::Deserialize)]
        struct ConnectResponse {
            auth_token: String,
        }

        let result: ConnectResponse = response
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

        info!("Token refreshed successfully for device {}", device_id);

        // 更新客户端身份
        self.client
            .set_identity(device_id, result.auth_token.clone())
            .await;

        // 更新本地 device_config 并持久化
        if let Some(ref mut device) = self.device_config {
            device.auth_token = result.auth_token.clone();
            if let Err(e) = device.save_to_file(&self.config.device_yaml_path(&device.server_url)) {
                warn!("Failed to persist refreshed token: {}", e);
            }
        }

        Ok(())
    }

    /// 等待转生（角色注册后触发重连）
    ///
    /// 当 agent_id 为 nil（未创建角色）或角色已归隐时，进入此等待循环。
    /// 保持进程存活，监听 reconnect_rx（Web 面板注册新角色后通过 HTTP API 触发）。
    pub(crate) async fn wait_for_rebirth(&mut self) -> Result<()> {
        info!(
            "Agent '{}' 进入等待转生模式，保持进程存活...",
            self.character_name()
        );

        loop {
            tokio::select! {
                // 监听重连请求（Web 面板注册新角色后触发）
                Ok(req) = async {
                    if let Some(ref mut rx) = self.reconnect_rx {
                        rx.recv().await
                    } else {
                        // 无 reconnect_rx（非 Claw/Cognitive HTTP API 模式），永远等待
                        std::future::pending().await
                    }
                } => {
                    info!("[rebirth] 收到重连请求: {}", req.ws_url);
                    let http_url = crate::config::ws_to_http_url(&req.ws_url);
                    self.client.update_server_url(req.ws_url.clone(), http_url).await;

                    match self.reconnect().await {
                        Ok(()) => {
                            info!("[rebirth] 重连成功，退出等待转生模式");
                            // reconnect 成功后 death_reported 已重置
                            return Ok(());
                        }
                        Err(e) => {
                            warn!("[rebirth] 重连失败: {}，继续等待", e);
                        }
                    }
                }
            }
        }
    }
}

/// 保存角色配置到磁盘
pub(crate) fn save_character_config_to_fs(
    config: &CharacterConfig,
    characters_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let agent_id = config
        .agent_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let dir = characters_dir.join(&agent_id);
    std::fs::create_dir_all(&dir)?;
    config.save_to_file(dir.join("character.yaml"))
}
