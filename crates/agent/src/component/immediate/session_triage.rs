//! SessionTriageEngine: 每游戏日一个后台 LLM triage 任务
//!
//! 监听 Notify 信号 → debounce 收集窗口 → 批量 LLM triage。
//! 游戏日结束时生成当日摘要，写入 episodic memory。

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use cyber_jianghu_protocol::{EventTriageConfig, EventTriagePreFilter};

use crate::component::llm::LlmClientExt;
use crate::runtime::claw::LlmClientContainer;
use crate::soul::reflector::PersonaInfo;

use super::event_store::{EventStore, StoredEvent, TriageDecision};

// ============================================================================
// LLM Triage 输出格式
// ============================================================================

/// LLM triage 批量输出的 JSON 格式
#[derive(Debug, Clone, Deserialize)]
struct TriageLlmOutput {
    triage: Vec<TriageItem>,
    #[allow(dead_code)]
    summary: Option<String>,
}

/// LLM triage 单条输出
#[derive(Debug, Clone, Deserialize)]
struct TriageItem {
    event_id: String,
    decision: String, // urgent / batch / ignore
    reason: String,
}

// ============================================================================
// SessionTriageEngine
// ============================================================================

/// 会话 Triage 引擎（每游戏日一个实例）
///
/// 由 `tokio::spawn` 运行为后台任务。
/// lifecycle.rs 持有 `JoinHandle`，负责生命周期管理。
pub struct SessionTriageEngine {
    /// EventStore 引用
    event_store: Arc<EventStore>,

    /// LLM 客户端容器（共享，与主 LLM 共用）
    llm_container: LlmClientContainer,

    /// 角色人设
    persona: PersonaInfo,

    /// 角色名称
    agent_name: String,

    /// triage 配置
    config: EventTriageConfig,

    /// 当前游戏日
    game_day: i64,

    /// lifecycle.rs 更新，session 读取
    current_game_day: Arc<RwLock<i64>>,

    /// batch_id 计数器
    next_batch_id: i64,
}

impl SessionTriageEngine {
    /// 创建新的 Session Triage Engine
    pub fn new(
        event_store: Arc<EventStore>,
        llm_container: LlmClientContainer,
        persona: PersonaInfo,
        agent_name: String,
        config: EventTriageConfig,
        game_day: i64,
        current_game_day: Arc<RwLock<i64>>,
    ) -> Self {
        Self {
            event_store,
            llm_container,
            persona,
            agent_name,
            config,
            game_day,
            current_game_day,
            next_batch_id: 1,
        }
    }

    /// 主循环（tokio::spawn 为后台任务）
    ///
    /// 监听 Notify 信号 + 兜底轮询 → debounce → 批量 triage。
    /// 游戏日结束时返回当日摘要，由 lifecycle 负责存储和提交。
    pub async fn run(mut self) -> Option<String> {
        let poll_interval = Duration::from_secs(self.config.poll_interval_secs);
        let debounce = Duration::from_secs(self.config.debounce_secs);
        let llm_timeout = Duration::from_millis(self.config.triage_llm_timeout_ms);
        let notify = self.event_store.notify().clone();

        info!(
            "SessionTriageEngine 启动: agent={}, game_day={}, poll={}s, debounce={}s",
            self.agent_name,
            self.game_day,
            self.config.poll_interval_secs,
            self.config.debounce_secs
        );

        let mut summary = None;

        loop {
            // 阶段 1：等待唤醒信号，或兜底轮询
            tokio::select! {
                _ = notify.notified() => {
                    // 收到信号 → debounce 收集窗口
                    tokio::time::sleep(debounce).await;
                }
                _ = tokio::time::sleep(poll_interval) => {}
            }

            // 阶段 2：查询待处理事件
            let pending = match self.event_store.query_pending_async(self.game_day).await {
                Ok(events) => events,
                Err(e) => {
                    error!("Session triage 查询 pending 事件失败: {}", e);
                    continue;
                }
            };

            if pending.is_empty() {
                // 阶段 5：游戏日结束检查
                if let Some(s) = self.check_game_day_ended().await {
                    summary = Some(s);
                    break;
                }
                continue;
            }

            debug!(
                "Session triage: {} 条 pending 事件 (game_day={})",
                pending.len(),
                self.game_day
            );

            // 阶段 3：批量 LLM triage（带超时）
            let decisions =
                match tokio::time::timeout(llm_timeout, self.triage_batch(&pending)).await {
                    Ok(Ok(decisions)) => decisions,
                    Ok(Err(e)) => {
                        warn!("Session triage LLM 调用失败: {}，使用规则兜底", e);
                        Self::fallback_priority_split(&pending, &self.config.pre_filter)
                    }
                    Err(_) => {
                        warn!(
                            "Session triage LLM 超时（{}ms），使用规则兜底",
                            self.config.triage_llm_timeout_ms
                        );
                        Self::fallback_priority_split(&pending, &self.config.pre_filter)
                    }
                };

            // 阶段 4：写回 DB
            let batch_id = self.next_batch_id;
            self.next_batch_id += 1;

            if let Err(e) = self
                .event_store
                .update_triage_async(decisions, batch_id)
                .await
            {
                error!("Session triage 写入 DB 失败: {}", e);
            }

            // 阶段 5：游戏日结束检查
            if let Some(s) = self.check_game_day_ended().await {
                summary = Some(s);
                break;
            }
        }

        info!(
            "SessionTriageEngine 退出: agent={}, game_day={}, has_summary={}",
            self.agent_name, self.game_day, summary.is_some()
        );

        summary
    }

    /// 检查游戏日是否已翻页，返回摘要（若已翻页）
    async fn check_game_day_ended(&self) -> Option<String> {
        let latest_day = *self.current_game_day.read().await;
        if latest_day != self.game_day {
            match self.produce_daily_summary().await {
                Ok(summary) => {
                    info!(
                        "游戏日 {} 摘要生成完成 ({} 字符)",
                        self.game_day,
                        summary.len()
                    );
                    Some(summary)
                }
                Err(e) => {
                    error!(
                        "游戏日 {} 摘要生成失败: {}，事件保留在 DB 中待清理",
                        self.game_day, e
                    );
                    None
                }
            }
        } else {
            None
        }
    }

    /// 批量 triage：一次 LLM 调用处理 N 条事件
    async fn triage_batch(&self, events: &[StoredEvent]) -> anyhow::Result<Vec<TriageDecision>> {
        let prompt = self.build_triage_prompt(events);
        let system = format!(
            "你是{}的「事件秘书」，负责为{}筛选周围发生的事件，判断哪些需要{}立即关注。只返回 JSON。",
            self.agent_name, self.agent_name, self.agent_name
        );

        let llm = self.llm_container.read().await;
        let llm_ref = llm.clone();
        drop(llm);

        let result: TriageLlmOutput = llm_ref
            .complete_json_with_system(&system, &prompt)
            .await
            .map_err(|e| anyhow::anyhow!("LLM triage 调用失败: {}", e))?;

        // 校验 + 转换
        let decisions: Vec<TriageDecision> = result
            .triage
            .into_iter()
            .map(|item| {
                let decision = match item.decision.to_lowercase().as_str() {
                    "urgent" => "urgent",
                    "batch" => "batch",
                    _ => "ignored",
                };
                TriageDecision {
                    event_id: item.event_id,
                    decision: decision.to_string(),
                    reason: item.reason,
                }
            })
            .collect();

        Ok(decisions)
    }

    /// 构建 triage prompt
    fn build_triage_prompt(&self, events: &[StoredEvent]) -> String {
        let personality = self.personality_str();

        let event_lines: Vec<String> = events
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let sender = e.from_agent_name.as_deref().unwrap_or("某人");
                format!(
                    "{}. [{}] {}「{}」",
                    i + 1,
                    e.event_type.as_str(),
                    sender,
                    e.description
                )
            })
            .collect();

        let event_ids: Vec<String> = events.iter().map(|e| e.event_id.clone()).collect();

        format!(
            r#"{name}的性格：{personality}

以下 {count} 条事件在{name}附近发生：
{events}

逐条判断紧急程度：
- urgent：需要{name}立即关注（如死亡、直接对话、威胁）
- batch：可以稍后了解（如闲聊、环境变化）
- ignore：与当前无关

返回 JSON：
{{"triage": [{{"event_id": "...", "decision": "urgent|batch|ignore", "reason": "简短理由"}}], "summary": "一句话概括当前场景"}}

event_id 必须是以下值之一：{event_ids}"#,
            name = self.agent_name,
            personality = personality,
            count = events.len(),
            events = event_lines.join("\n"),
            event_ids = event_ids.join(", "),
        )
    }

    fn personality_str(&self) -> String {
        let mut parts = Vec::new();
        if !self.persona.personality.is_empty() {
            parts.push(self.persona.personality.join("、"));
        }
        if !self.persona.values.is_empty() {
            parts.push(format!("信奉{}", self.persona.values.join("、")));
        }
        if parts.is_empty() {
            "江湖中人".to_string()
        } else {
            parts.join("，")
        }
    }

    /// 超时兜底：按 priority 分流
    ///
    /// priority >= 80 → urgent，其余 → batch
    pub fn fallback_priority_split(
        events: &[StoredEvent],
        config: &EventTriagePreFilter,
    ) -> Vec<TriageDecision> {
        events
            .iter()
            .map(|e| {
                let priority = config
                    .event_type_priority
                    .get(&e.event_type)
                    .copied()
                    .unwrap_or(config.default_priority);
                let decision = if priority >= 80 { "urgent" } else { "batch" };
                TriageDecision {
                    event_id: e.event_id.clone(),
                    decision: decision.to_string(),
                    reason: format!("LLM超时-规则兜底(priority={})", priority),
                }
            })
            .collect()
    }

    /// 游戏日结束：生成摘要
    async fn produce_daily_summary(&self) -> anyhow::Result<String> {
        // 查询当日所有已 triage 的事件
        let triaged = self
            .event_store
            .query_triaged_async(self.config.context.clone())
            .await?;

        let total_count = triaged.urgent.len() + triaged.batch.len();

        if total_count == 0 {
            return Ok(format!("游戏日 {}：平静无事。", self.game_day));
        }

        // 构建摘要
        let urgent_desc: Vec<&str> = triaged
            .urgent
            .iter()
            .map(|e| e.description.as_str())
            .collect();
        let batch_desc: Vec<&str> = triaged
            .batch
            .iter()
            .map(|e| e.description.as_str())
            .take(10) // 最多取 10 条
            .collect();

        let summary = format!(
            "游戏日 {} 摘要：{} 条紧急事件（{}），{} 条一般事件（{}...）",
            self.game_day,
            triaged.urgent.len(),
            urgent_desc.join("；"),
            triaged.batch.len(),
            batch_desc.join("；"),
        );

        // 摘要通过 run() 返回值交给 lifecycle 处理（episodic 存储 + server 提交）
        info!("游戏日 {} 摘要: {}", self.game_day, summary);

        // 清理过期事件
        if let Err(e) = self.event_store.cleanup_old_async(self.game_day).await {
            warn!("清理过期事件失败: {}", e);
        }

        Ok(summary)
    }
}
