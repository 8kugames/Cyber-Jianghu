use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;

use super::super::Agent;
use crate::models::{WorldEvent, WorldEventType};

/// 在 events_log 中查找「自身」的死亡事件。
///
/// 服务端把同位置他人的死亡（目击事件，WitnessedDeath）与自身死亡以同一种
/// `WorldEventType::DeathNotification` 投递进 events_log，唯一区别是
/// `metadata.agent_id` 记录死者 ID。必须比对死者与自身：目击者若被误判为
/// 自身死亡，is_dead 置位后无自愈路径（server auto-rebirth 按
/// status='dead' 守卫拒绝活体重生），将永久停在决策跳过循环。
///
/// 身份无法核对时（自身 ID 未知、事件未携带死者 ID）不触发死亡报告：
/// 误报（假死）不可逆，漏报只损失一次提前上报，AgentDied 回调（路径 2）
/// 仍会兜底。
pub(super) fn find_self_death(
    events_log: &[WorldEvent],
    self_agent_id: Option<Uuid>,
) -> Option<&WorldEvent> {
    let self_id = self_agent_id?;
    let self_id_str = self_id.to_string();
    events_log.iter().find(|e| {
        e.event_type == WorldEventType::DeathNotification
            && e.metadata.get("agent_id").and_then(|v| v.as_str()) == Some(self_id_str.as_str())
    })
}

struct RebirthParams {
    old_agent_id: Uuid,
    delay_ms: u64,
    http_url: String,
    api_state: Arc<crate::infra::api::HttpApiState>,
    device_id: Uuid,
    auth_token: String,
    retry_max: u32,
    retry_interval: std::time::Duration,
    context: String,
}

fn schedule_auto_rebirth(params: RebirthParams) {
    let RebirthParams {
        old_agent_id,
        delay_ms,
        http_url,
        api_state,
        device_id,
        auth_token,
        retry_max,
        retry_interval,
        context,
    } = params;
    let context_label = context.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
        info!(
            "自动转世重生{}: 调用 auto-rebirth API (old_agent={})",
            context_label, old_agent_id
        );

        let client = reqwest::Client::new();
        let url = format!("{}/api/v1/agent/auto-rebirth", http_url);
        let body = serde_json::json!({
            "device_id": device_id,
            "auth_token": auth_token,
            "old_agent_id": old_agent_id,
        });

        for attempt in 0..retry_max {
            match client.post(&url).json(&body).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let data: serde_json::Value = resp.json().await.unwrap_or_default();
                    let new_id = data["new_agent_id"]
                        .as_str()
                        .and_then(|s| s.parse::<Uuid>().ok())
                        .unwrap_or(Uuid::nil());
                    let system_prompt = data["system_prompt"].as_str().map(ToOwned::to_owned);

                    info!(
                        "自动转世重生成功: old_agent={} → new_agent={}",
                        old_agent_id, new_id
                    );

                    *api_state.pending_rebirth_agent_id.write().await = Some(new_id);
                    *api_state.pending_rebirth_system_prompt.write().await = system_prompt;
                    api_state
                        .is_dead
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    api_state.rebirth_notify.notify_waiters();
                    return;
                }
                Ok(resp) => {
                    let status = resp.status();
                    let resp_body = resp.text().await.unwrap_or_default();
                    warn!(
                        "自动转世重生服务端拒绝 (attempt {}/{}): status={}, body={}",
                        attempt + 1,
                        retry_max,
                        status,
                        resp_body
                    );
                }
                Err(e) => {
                    warn!(
                        "自动转世重生网络错误 (attempt {}/{}): {}",
                        attempt + 1,
                        retry_max,
                        e
                    );
                }
            }
            if attempt + 1 < retry_max {
                tokio::time::sleep(retry_interval).await;
            }
        }
        tracing::error!(
            "自动转世重生最终失败{}: old_agent={}, 所有 {} 次重试用尽",
            context_label,
            old_agent_id,
            retry_max
        );
    });
}

pub(super) async fn maybe_schedule_auto_rebirth(
    agent: &Agent,
    dead_agent_id: Uuid,
    _dead_tick_id: i64,
    context: &str,
) {
    let auto_rebirth_enabled = agent
        .http_api_state
        .as_ref()
        .map(|s| s.auto_rebirth.load(std::sync::atomic::Ordering::Relaxed))
        .unwrap_or(true);

    // 重生延迟回退默认：对齐 server game_rules.yaml 出厂值（delay_ticks: 5）。
    // 仅在 game_rules 尚未到达（新鲜进程注册即 nil）且无动态值时使用；
    // game_rules 已到达且显式为 0（= 不自动重生）时仍尊重用户配置。
    const FALLBACK_REBIRTH_DELAY_TICKS: i32 = 5;

    // 重生延迟来源链: (1) WS AgentDied 动态覆写 > (2) 注册时 game_rules 默认值
    // 两条死亡检测路径（events_log vs WS 回调）解耦: 不依赖 WS 回调写入时序。
    let effective_delay = if agent.rebirth_delay_ticks > 0 {
        agent.rebirth_delay_ticks
    } else {
        let cfg_delay = agent.config.rebirth_delay_ticks();
        if cfg_delay > 0 { cfg_delay } else { 0 }
    };

    // game_rules 缺失时 rebirth_delay_ticks() 返回 0，与"显式配置 0 = 不自动重生"
    // 无法区分。回退出厂默认，避免"容器重启 + 角色已死"静默卡死在等待转生模式。
    let effective_delay = if effective_delay <= 0 && agent.config.game_rules.is_none() {
        warn!(
            "rebirth_delay_ticks 未知（game_rules 未到达），回退出厂默认 {} ticks",
            FALLBACK_REBIRTH_DELAY_TICKS
        );
        FALLBACK_REBIRTH_DELAY_TICKS
    } else {
        effective_delay
    };

    if effective_delay <= 0 || !auto_rebirth_enabled {
        return;
    }

    let delay_ticks = effective_delay;
    let tick_secs = agent.get_tick_duration().await.as_secs();
    let delay_ms = delay_ticks as u64 * tick_secs * 1000;

    let old_agent_id = dead_agent_id;

    let http_url = agent.config.server.http_url.clone();
    let Some(api_state) = agent.http_api_state.clone() else {
        return;
    };

    let Some(device_cfg) = agent.device_config.as_ref() else {
        warn!("自动转世重生跳过: device_config 未设置");
        return;
    };
    let device_id = device_cfg.device_id;
    let auth_token = device_cfg.auth_token.clone();

    let retry_max = agent
        .config
        .game_rules
        .as_ref()
        .map(|r| r.rebirth_retry_max_attempts)
        .unwrap_or(3);
    let retry_interval = std::time::Duration::from_secs(
        agent
            .config
            .game_rules
            .as_ref()
            .map(|r| r.rebirth_retry_interval_secs)
            .unwrap_or(30),
    );

    if old_agent_id == Uuid::nil() {
        warn!(
            "自动转世重生跳过: 无法获取有效的 old_agent_id \
             (agent_id=None, api_state.agent_id=None)"
        );
        return;
    }

    info!(
        "自动转世重生已调度{}: agent={}, delay={} ticks ({}s)",
        context,
        old_agent_id,
        delay_ticks,
        delay_ms / 1000
    );

    schedule_auto_rebirth(RebirthParams {
        old_agent_id,
        delay_ms,
        http_url,
        api_state,
        device_id,
        auth_token,
        retry_max,
        retry_interval,
        context: context.to_string(),
    });
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::find_self_death;
    use crate::component::persona::event_mapper::EventContext;
    use crate::component::persona::rules_loader::load_event_trait_rules;
    use crate::models::{WorldEvent, WorldEventType};
    use uuid::Uuid;

    fn death_event(deceased: Option<Uuid>, description: &str) -> WorldEvent {
        WorldEvent {
            event_type: WorldEventType::DeathNotification,
            tick_id: 1,
            description: description.to_string(),
            metadata: match deceased {
                Some(id) => serde_json::json!({ "agent_id": id.to_string() }),
                None => serde_json::json!({}),
            },
        }
    }

    fn other_event() -> WorldEvent {
        WorldEvent {
            event_type: WorldEventType::ActionResult,
            tick_id: 1,
            description: "采集了野草".to_string(),
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn witnessed_death_of_other_is_not_self_death() {
        // P0 回归锁定：目击他人死亡不得触发自身死亡报告（否则目击者永久假死）
        let self_id = Uuid::new_v4();
        let other_id = Uuid::new_v4();
        let log = vec![
            other_event(),
            death_event(Some(other_id), "张三在 龙门大堂 亡故：饥渴交加，体力不支"),
        ];
        assert!(
            find_self_death(&log, Some(self_id)).is_none(),
            "目击他人死亡不得被判为自身死亡"
        );
    }

    #[test]
    fn own_death_in_events_log_is_detected() {
        let self_id = Uuid::new_v4();
        let log = vec![other_event(), death_event(Some(self_id), "你已亡故")];
        let found = find_self_death(&log, Some(self_id)).expect("自身死亡必须被检测到");
        assert_eq!(found.description, "你已亡故");
    }

    #[test]
    fn mixed_log_finds_self_death_not_witnessed() {
        let self_id = Uuid::new_v4();
        let log = vec![
            death_event(Some(Uuid::new_v4()), "张三亡故"),
            death_event(Some(self_id), "你亡故"),
        ];
        assert_eq!(
            find_self_death(&log, Some(self_id)).map(|e| e.description.as_str()),
            Some("你亡故"),
            "同 tick 既有目击又有自身死亡时，必须定位自身那条"
        );
    }

    #[test]
    fn death_event_without_deceased_id_is_ignored() {
        // fail-safe：身份无法核对不触发死亡。误报（假死）不可逆，
        // 漏报由 AgentDied WS 回调（路径 2）兜底。
        let self_id = Uuid::new_v4();
        let log = vec![death_event(None, "有人亡故")];
        assert!(find_self_death(&log, Some(self_id)).is_none());
    }

    #[test]
    fn unknown_self_identity_is_ignored() {
        let other_id = Uuid::new_v4();
        let log = vec![death_event(Some(other_id), "张三亡故")];
        assert!(find_self_death(&log, None).is_none());
    }

    #[test]
    fn witnessed_death_still_drives_trait_evolution() {
        // 目击合同另一半：不假死（上列测试），但必须被 WitnessedDeath 规则影响——
        // 事件经 classify_event 进入特质演化，而非被丢弃。
        let other_id = Uuid::new_v4();
        let log = vec![death_event(
            Some(other_id),
            "张三在 龙门大堂 亡故：饥渴交加，体力不支",
        )];
        assert!(
            find_self_death(&log, Some(Uuid::new_v4())).is_none(),
            "目击者不假死"
        );

        let yaml_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("crates/server/config/persona_event_rules.yaml");
        let mapper = load_event_trait_rules(&yaml_path).expect("YAML 28 规则必须可加载");
        let event = &log[0];
        assert_eq!(
            EventContext::classify_event(event),
            crate::component::persona::event_mapper::EventType::WitnessedDeath,
            "目击死亡必须分类为 WitnessedDeath"
        );

        let agent_id = Uuid::new_v4();
        let mut persona =
            crate::component::persona::DynamicPersona::new(agent_id, "目击者", "基础描述");
        persona.set_trait("恐惧", 10);
        persona.set_trait("沮丧", 10);
        mapper.apply_to_persona(event, &mut persona, 1);
        assert!(
            persona.get_trait("恐惧").unwrap() > 10,
            "目击死亡必须提升恐惧特质"
        );
        assert!(
            persona.get_trait("沮丧").unwrap() > 10,
            "目击死亡必须提升沮丧特质"
        );
    }
}
