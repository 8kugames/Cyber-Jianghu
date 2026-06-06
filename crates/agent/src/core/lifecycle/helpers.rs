// ============================================================================
// Lifecycle 辅助方法
// ============================================================================
//
// 独立的工具函数，不依赖 run() 的局部状态

use anyhow::Result;
use cyber_jianghu_protocol::{CalendarConfig, Entity, WorldTime, game_day_from_world_time};
use tracing::{info, warn};
use uuid::Uuid;

use crate::models::Intent;

impl super::super::Agent {
    /// 从 WorldTime 计算游戏日（用于 EventStore game_day 字段）
    ///
    /// 数据驱动：从 CalendarConfig (time.yaml) 读取 days_per_season / seasons_per_year。
    /// 算法统一在协议层 `game_day_from_world_time`，
    /// 此处仅在缺失 calendar 时使用排序键兜底。
    pub(super) fn compute_game_day(time: &WorldTime, calendar: Option<&CalendarConfig>) -> i64 {
        match calendar {
            Some(cal) => game_day_from_world_time(time, cal),
            None => {
                // 降级：无 calendar 配置时（旧服务器），用单调排序键避免碰撞
                time.year as i64 * 10000 + time.month as i64 * 100 + time.day as i64
            }
        }
    }

    pub async fn close(&mut self) -> Result<()> {
        // 终止 SessionTriageEngine 后台任务
        if let Some(handle) = self.session_triage_handle.take() {
            handle.abort();
        }
        if let Some(ref store) = self.persona_store
            && store.config_flush_on_shutdown()
        {
            let tick_id = self.current_tick.load(std::sync::atomic::Ordering::Relaxed);
            if let Err(e) = self.persona.read(|p| store.snapshot_now(p, tick_id)) {
                warn!("persona 退出 flush 失败: {}", e);
            }
        }
        self.client.close().await;
        info!("Agent '{}' stopped", self.character_name());
        Ok(())
    }

    /// 序列化 WorldTime 为 JSON 存储（展示由 server 预格式化的 `world_time_text` 字段负责）
    pub(super) fn format_world_time(wt: &WorldTime) -> String {
        serde_json::to_string(wt).unwrap_or_else(|_| wt.to_chinese())
    }

    /// LLM 失败时的 chaos fallback：尝试生成生存导向 intent，失败则退回休息
    pub(super) fn chaos_fallback_intent(
        &mut self,
        world_state: &cyber_jianghu_protocol::WorldState,
        agent_id: Uuid,
        fallback_thought: String,
    ) -> Intent {
        if let Some(ref mut generator) = self.chaos_generator {
            let actions: Vec<_> = self
                .config
                .game_rules
                .as_ref()
                .map(|g| g.available_actions.clone())
                .unwrap_or_default();
            if !actions.is_empty() {
                let chaos_intents = generator.generate_llm_chaos_intents(
                    world_state,
                    &actions,
                    1,
                    self.consecutive_llm_failures as usize,
                );
                if let Some(intent) = chaos_intents.into_iter().next() {
                    info!(
                        "Chaos fallback: agent={}, action={}",
                        self.character_name(),
                        intent.action_type
                    );
                    return intent;
                }
            }
        }
        // chaos 不可用 → 绝对兜底休息
        warn!(
            "Chaos fallback 不可用，退回休息: agent={}",
            self.character_name()
        );
        Intent::new(agent_id, world_state.tick_id, "休息", None).with_thought(fallback_thought)
    }

    /// 将 action_type + action_data 生成可读简述
    pub(super) fn summarize_intent(
        action_type: &str,
        action_data: Option<&serde_json::Value>,
        location: &str,
        entities: &[Entity],
    ) -> String {
        let data = action_data.cloned().unwrap_or(serde_json::Value::Null);

        let resolve_name = |target_id: &str| -> String {
            if let Ok(uuid) = target_id.parse::<uuid::Uuid>()
                && let Some(entity) = entities.iter().find(|e| e.id == uuid)
            {
                return format!("{}（{}）", entity.name, &target_id[..8]);
            }
            target_id.chars().take(8).collect::<String>() + "..."
        };

        match action_type {
            "说话" => {
                let content = data.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let target = data.get("target_agent_id").and_then(|v| v.as_str());
                match target {
                    Some(tid) => format!("对{}说话：{}", resolve_name(tid), content),
                    None => format!("向在场众人说话：{}", content),
                }
            }
            "私语" => {
                let content = data.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let target = data.get("target_agent_id").and_then(|v| v.as_str());
                match target {
                    Some(tid) => format!("向{}密语：{}", resolve_name(tid), content),
                    None => format!("向某人密语：{}", content),
                }
            }
            "大喊" => {
                let content = data.get("content").and_then(|v| v.as_str()).unwrap_or("");
                format!("大声喊道：{}", content)
            }
            "移动" => {
                let target = data
                    .get("target_location")
                    .and_then(|v| v.as_str())
                    .unwrap_or("未知地点");
                format!("从{}移动到{}", location, target)
            }
            "进食" => {
                let item = data
                    .get("item_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("食物");
                format!("吃了{}", item)
            }
            "饮水" => {
                let item = data.get("item_id").and_then(|v| v.as_str()).unwrap_or("水");
                format!("喝了{}", item)
            }
            "采集" => {
                let resource = data
                    .get("target_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("资源");
                format!("采集{}", resource)
            }
            "拾取" => {
                let item = data
                    .get("item_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("物品");
                format!("拾起{}", item)
            }
            "给予" => {
                let item = data
                    .get("item_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("物品");
                format!("给予{}", item)
            }
            "休息" => "原地休息".to_string(),
            other => format!("执行{}", other),
        }
    }

    /// 启动时主动从 Server 拉取 prompt_templates 并写盘
    ///
    /// 确保本地存在 prompt_templates.json 文件供下次冷启动使用。
    /// 失败不阻塞启动——WS ConfigUpdate 已在连接时更新了 runtime config。
    pub(super) async fn fetch_prompt_templates_from_server(&self) {
        let Some(ref engine) = self.cognitive_engine else {
            return;
        };
        let Some(ref device_cfg) = self.device_config else {
            return;
        };

        let http_url = self.config.server.http_url.clone();
        let device_id = device_cfg.device_id;
        let auth_token = device_cfg.auth_token.clone();
        let engine = engine.clone();

        let client = reqwest::Client::new();
        let url = format!("{}/api/v1/agent/prompt-templates", http_url);
        let body = serde_json::json!({
            "device_id": device_id,
            "auth_token": auth_token,
        });

        match client.post(&url).json(&body).send().await {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<serde_json::Value>().await {
                    Ok(data) => {
                        let hash = data["hash"].as_str().unwrap_or("");
                        let version = data["version"].as_str().unwrap_or("");
                        if let Some(content) = data.get("content") {
                            match cyber_jianghu_protocol::PromptTemplateConfig::from_json_value(
                                content.clone(),
                            ) {
                                Ok(config) => {
                                    info!(
                                        "启动拉取 prompt_templates 成功: version={}, hash={}",
                                        version,
                                        &hash[..12.min(hash.len())]
                                    );
                                    engine.update_prompt_template_from_config(config);
                                    engine.save_prompt_template_to_disk();
                                }
                                Err(e) => {
                                    warn!("启动拉取 prompt_templates 解析失败: {}", e);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("启动拉取 prompt_templates 响应解析失败: {}", e);
                    }
                }
            }
            Ok(resp) => {
                warn!("启动拉取 prompt_templates 失败: status={}", resp.status());
            }
            Err(e) => {
                warn!("启动拉取 prompt_templates 请求失败: {}", e);
            }
        }
    }
}
