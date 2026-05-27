//! SessionTriageEngine: 每游戏日一个后台 LLM triage 任务
//!
//! 监听 Notify 信号 → debounce 收集窗口 → 批量 LLM triage。
//! 游戏日结束时生成当日摘要，写入 episodic memory。

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use cyber_jianghu_protocol::{EventTriageConfig, EventTriagePreFilter, WorldTime};

use crate::component::llm::LlmClientExt;
use crate::component::memory::backend::SearchableBackend;
use crate::component::memory::MemoryManager;
use crate::component::social::RelationshipStore;
use crate::component::state_store::WorldStateStore;
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

    /// 世界时间（用于天道历日期格式化）
    world_time: Option<WorldTime>,

    // ── 日记数据源（component 层，非 soul 层） ──

    /// 情景记忆管理器（获取今日重要记忆）
    memory_manager: Option<Arc<RwLock<MemoryManager>>>,

    /// 社交关系存储（获取关系快照）
    relationship_store: Option<RelationshipStore>,

    /// 世界状态存储（获取附近实体、位置）
    world_state_store: Option<Arc<WorldStateStore>>,

    /// 日记提示词模板（从 prompt_templates.yaml 的 daily_diary 条目加载）
    diary_prompt: Option<String>,
}

impl SessionTriageEngine {
    /// 创建新的 Session Triage Engine
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        event_store: Arc<EventStore>,
        llm_container: LlmClientContainer,
        persona: PersonaInfo,
        agent_name: String,
        config: EventTriageConfig,
        game_day: i64,
        current_game_day: Arc<RwLock<i64>>,
        world_time: Option<WorldTime>,
        memory_manager: Option<Arc<RwLock<MemoryManager>>>,
        relationship_store: Option<RelationshipStore>,
        world_state_store: Option<Arc<WorldStateStore>>,
        diary_prompt: Option<String>,
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
            world_time,
            memory_manager,
            relationship_store,
            world_state_store,
            diary_prompt,
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

        #[allow(unused_assignments)]
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

            // 阶段 2：立即检测游戏日是否已切换（不论有无事件）
            if let Some(s) = self.check_game_day_ended().await {
                summary = Some(s);
                break;
            }

            // 阶段 3：查询待处理事件
            let pending = match self.event_store.query_pending_async(self.game_day).await {
                Ok(events) => events,
                Err(e) => {
                    error!("Session triage 查询 pending 事件失败: {}", e);
                    continue;
                }
            };

            if pending.is_empty() {
                // 无事件时 sleep 避免空轮询（仅靠 select 的 poll_interval 不足）
                tokio::time::sleep(poll_interval).await;
                continue;
            }

            debug!(
                "Session triage: {} 条 pending 事件 (game_day={})",
                pending.len(),
                self.game_day
            );

            // 阶段 4：批量 LLM triage（带超时）
            let decisions =
                match tokio::time::timeout(llm_timeout, self.triage_batch(&pending)).await {
                    Ok(Ok(decisions)) => decisions,
                    Ok(Err(e)) => {
                        warn!("Session triage LLM 调用失败: {}，使用规则兜底", e);
                        Self::fallback_priority_split_error(&pending, &self.config.pre_filter)
                    }
                    Err(_) => {
                        warn!(
                            "Session triage LLM 超时（{}ms），使用规则兜底",
                            self.config.triage_llm_timeout_ms
                        );
                        Self::fallback_priority_split_timeout(&pending, &self.config.pre_filter)
                    }
                };

            // 阶段 5：写回 DB
            let batch_id = self.next_batch_id;
            self.next_batch_id += 1;

            if let Err(e) = self
                .event_store
                .update_triage_async(decisions, batch_id)
                .await
            {
                error!("Session triage 写入 DB 失败: {}", e);
            }

            // 阶段 6：游戏日结束检查（triage 后也检测，确保事件处理完后立即响应 day 切换）
            if let Some(s) = self.check_game_day_ended().await {
                summary = Some(s);
                break;
            }
        }

        info!(
            "SessionTriageEngine 退出: agent={}, game_day={}, has_summary={}",
            self.agent_name,
            self.game_day,
            summary.is_some()
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

    pub fn fallback_priority_split(
        events: &[StoredEvent],
        config: &EventTriagePreFilter,
    ) -> Vec<TriageDecision> {
        Self::fallback_priority_split_timeout(events, config)
    }

    pub fn fallback_priority_split_timeout(
        events: &[StoredEvent],
        config: &EventTriagePreFilter,
    ) -> Vec<TriageDecision> {
        Self::fallback_priority_split_with_label(events, config, "LLM超时-规则兜底")
    }

    pub fn fallback_priority_split_error(
        events: &[StoredEvent],
        config: &EventTriagePreFilter,
    ) -> Vec<TriageDecision> {
        Self::fallback_priority_split_with_label(events, config, "LLM失败-规则兜底")
    }

    fn fallback_priority_split_with_label(
        events: &[StoredEvent],
        config: &EventTriagePreFilter,
        label: &str,
    ) -> Vec<TriageDecision> {
        let urgent_cutoff = config.fallback_urgent_cutoff_priority;
        let ignore_cutoff = config.fallback_ignore_cutoff_priority;
        events
            .iter()
            .map(|e| {
                let priority = config
                    .event_type_priority
                    .get(&e.event_type)
                    .copied()
                    .unwrap_or(config.default_priority);
                let decision = if priority >= urgent_cutoff {
                    "urgent"
                } else if priority < ignore_cutoff {
                    "ignored"
                } else {
                    "batch"
                };
                TriageDecision {
                    event_id: e.event_id.clone(),
                    decision: decision.to_string(),
                    reason: format!("{}(priority={})", label, priority),
                }
            })
            .collect()
    }

    /// 游戏日结束：日记路径（多数据源） → 纯事件回述降级
    async fn produce_daily_summary(&self) -> anyhow::Result<String> {
        if self.diary_prompt.is_some()
            && (self.memory_manager.is_some()
                || self.relationship_store.is_some()
                || self.world_state_store.is_some())
        {
            return self.produce_diary_with_stores().await;
        }
        self.produce_event_summary().await
    }

    /// 多数据源江湖日记（情景记忆+关系+世界状态+事件）
    async fn produce_diary_with_stores(&self) -> anyhow::Result<String> {
        // 1. 收集数据源
        let memories = self.query_diary_memories().await;
        let relationships = self.query_diary_relationships().await;
        let world_context = self.query_diary_world_context().await;

        // 2. 日期格式化
        let date_str = self
            .world_time
            .as_ref()
            .map(|wt| wt.to_chinese())
            .unwrap_or_else(|| format!("游戏日 {}", self.game_day));

        // 3. 事件数据（复用原有 triage 结果）
        let events_str = match self
            .event_store
            .query_triaged_async(self.config.context.clone(), self.game_day)
            .await
        {
            Ok(triaged) => {
                let all: Vec<String> = triaged
                    .urgent
                    .iter()
                    .chain(triaged.batch.iter())
                    .map(|e| e.description.clone())
                    .collect();
                if all.is_empty() {
                    "无特殊事件".to_string()
                } else {
                    all.join("；")
                }
            }
            Err(_) => "无特殊事件".to_string(),
        };

        // 4. 从模板变量渲染提示词
        let template = match self.diary_prompt.as_ref() {
            Some(t) => t,
            None => return self.produce_event_summary().await,
        };

        let personality = self.personality_str();
        let prompt = template
            .replace("{agent_name}", &self.agent_name)
            .replace("{date}", &date_str)
            .replace("{personality}", &personality)
            .replace("{memories}", &memories)
            .replace("{relationships}", &relationships)
            .replace("{world_context}", &world_context)
            .replace("{events}", &events_str);

        // 5. 单次 LLM 调用
        let llm = self.llm_container.read().await;
        let llm_ref = llm.clone();
        drop(llm);

        let result: serde_json::Value = llm_ref
            .complete_json_with_system(&prompt, "")
            .await
            .map_err(|e| anyhow::anyhow!("LLM 日记生成失败: {}", e))?;

        let diary = result
            .get("diary")
            .and_then(|v| v.as_str())
            .unwrap_or("摘要生成失败")
            .to_string();

        // 6. 清理过期事件
        if let Err(e) = self.event_store.cleanup_old_async(self.game_day).await {
            warn!("清理过期事件失败: {}", e);
        }

        Ok(diary)
    }

    /// 查询当日重要记忆（重要性降序，上限 20 条）
    async fn query_diary_memories(&self) -> String {
        let mm = match self.memory_manager.as_ref() {
            Some(mm) => mm,
            None => return String::new(),
        };
        let guard = mm.read().await;
        match guard.episodic().get_top_by_importance(20).await {
            Ok(memories) if !memories.is_empty() => memories
                .iter()
                .enumerate()
                .map(|(i, m)| format!("{}. {}", i + 1, m.content))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        }
    }

    /// 查询社交关系快照
    async fn query_diary_relationships(&self) -> String {
        let store = match self.relationship_store.as_ref() {
            Some(s) => s,
            None => return String::new(),
        };
        match store.get_all_relationships() {
            Ok(rels) if !rels.is_empty() => rels
                .iter()
                .map(|r| {
                    let fav = if r.favorability >= 30 {
                        "友善"
                    } else if r.favorability <= -30 {
                        "敌对"
                    } else {
                        "中立"
                    };
                    format!("{}（{}，好感度{}）", r.target_name, fav, r.favorability)
                })
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        }
    }

    /// 查询世界状态上下文
    async fn query_diary_world_context(&self) -> String {
        let store = match self.world_state_store.as_ref() {
            Some(s) => s,
            None => return String::new(),
        };
        let ws = match store.current().await {
            Some(ws) => ws,
            None => return String::new(),
        };
        let mut parts = Vec::new();
        parts.push(format!("当前位置：{}", ws.location.name));
        if !ws.entities.is_empty() {
            let names: Vec<String> = ws
                .entities
                .iter()
                .map(|e| e.name.clone())
                .filter(|n| !n.is_empty())
                .collect();
            if !names.is_empty() {
                parts.push(format!("附近的人：{}", names.join("、")));
            }
        }
        parts.join("\n")
    }

    /// 游戏日结束：纯事件回述（diary 的降级路径）
    async fn produce_event_summary(&self) -> anyhow::Result<String> {
        // 查询当日所有已 triage 的事件（按 game_day 过滤）
        let triaged = self
            .event_store
            .query_triaged_async(self.config.context.clone(), self.game_day)
            .await?;

        let total_count = triaged.urgent.len() + triaged.batch.len();

        // 日期格式化：使用天道历
        let date_str = self
            .world_time
            .as_ref()
            .map(|wt| wt.to_chinese())
            .unwrap_or_else(|| format!("游戏日 {}", self.game_day));

        if total_count == 0 {
            return Ok(format!("{}：平静无事。", date_str));
        }

        // 构建事件描述
        let urgent_events: Vec<&str> = triaged
            .urgent
            .iter()
            .map(|e| e.description.as_str())
            .collect();
        let batch_events: Vec<&str> = triaged
            .batch
            .iter()
            .map(|e| e.description.as_str())
            .take(10)
            .collect();

        // LLM生成叙事化摘要
        let prompt = format!(
            r#"你是{agent_name}的史官，为{date_str}撰写江湖起居注。

当日他人交互：
紧急事件（{urgent_count}条）：{urgent_events}
一般事件（{batch_count}条）：{batch_events}

要求：
1. 以"我"为中心视角
2. 语言古朴典雅，武侠风格
3. 400-600字，纯叙事散文
4. 叙事化整合，不要事件计数

返回JSON：{{"narrative": "..."}}"#,
            agent_name = self.agent_name,
            date_str = date_str,
            urgent_count = triaged.urgent.len(),
            urgent_events = urgent_events.join("；"),
            batch_count = triaged.batch.len(),
            batch_events = batch_events.join("；")
        );

        let llm = self.llm_container.read().await;
        let llm_ref = llm.clone();
        drop(llm);

        let result: serde_json::Value = llm_ref
            .complete_json_with_system(
                &format!(
                    "你是{agent_name}的史官，为{date_str}撰写江湖起居注。",
                    agent_name = self.agent_name,
                    date_str = date_str
                ),
                &prompt,
            )
            .await
            .map_err(|e| anyhow::anyhow!("LLM摘要生成失败: {}", e))?;

        let narrative = result
            .get("narrative")
            .and_then(|v| v.as_str())
            .unwrap_or("摘要生成失败")
            .to_string();

        // 清理过期事件
        if let Err(e) = self.event_store.cleanup_old_async(self.game_day).await {
            warn!("清理过期事件失败: {}", e);
        }

        Ok(narrative)
    }
}

#[cfg(test)]
mod fallback_tests {
    use super::SessionTriageEngine;
    use crate::component::immediate::event_store::StoredEvent;
    use cyber_jianghu_protocol::{EventTriagePreFilter, WorldEventType};

    fn mk_event(event_id: &str, event_type: WorldEventType) -> StoredEvent {
        StoredEvent {
            id: 1,
            event_id: event_id.to_string(),
            event_type,
            from_agent_id: None,
            from_agent_name: None,
            description: "x".to_string(),
            metadata: "{}".to_string(),
            received_at_tick: 1,
            game_day: 1,
            triage_status: "pending".to_string(),
            triage_reason: None,
            triage_batch_id: None,
            processed_at_tick: None,
        }
    }

    #[test]
    fn fallback_priority_split_three_way() {
        let mut event_type_priority = std::collections::HashMap::new();
        event_type_priority.insert(WorldEventType::DeathNotification, 100);
        event_type_priority.insert(WorldEventType::PrivateDialogue, 80);
        event_type_priority.insert(WorldEventType::SocialInteraction, 60);
        event_type_priority.insert(WorldEventType::StateChange, 50);
        event_type_priority.insert(WorldEventType::ActionResult, 40);
        event_type_priority.insert(WorldEventType::PublicMessage, 20);
        event_type_priority.insert(WorldEventType::EnvironmentalChange, 10);
        event_type_priority.insert(WorldEventType::SystemNotification, 10);
        event_type_priority.insert(WorldEventType::TimeUpdate, 5);

        let pre = EventTriagePreFilter {
            fallback_urgent_cutoff_priority: 80,
            fallback_ignore_cutoff_priority: 20,
            max_events_per_triage: 50,
            default_priority: 0,
            event_type_priority,
        };

        let events = vec![
            mk_event("e1", WorldEventType::PrivateDialogue),
            mk_event("e2", WorldEventType::SocialInteraction),
            mk_event("e3", WorldEventType::TimeUpdate),
        ];

        let decisions = SessionTriageEngine::fallback_priority_split(&events, &pre);
        let mut map = std::collections::HashMap::new();
        for d in decisions {
            map.insert(d.event_id, d.decision);
        }

        assert_eq!(map.get("e1").unwrap(), "urgent");
        assert_eq!(map.get("e2").unwrap(), "batch");
        assert_eq!(map.get("e3").unwrap(), "ignored");
    }

    #[test]
    fn fallback_reason_distinguishes_error_and_timeout() {
        let mut event_type_priority = std::collections::HashMap::new();
        event_type_priority.insert(WorldEventType::DeathNotification, 100);
        event_type_priority.insert(WorldEventType::PrivateDialogue, 80);
        event_type_priority.insert(WorldEventType::SocialInteraction, 60);
        event_type_priority.insert(WorldEventType::StateChange, 50);
        event_type_priority.insert(WorldEventType::ActionResult, 40);
        event_type_priority.insert(WorldEventType::PublicMessage, 20);
        event_type_priority.insert(WorldEventType::EnvironmentalChange, 10);
        event_type_priority.insert(WorldEventType::SystemNotification, 10);
        event_type_priority.insert(WorldEventType::TimeUpdate, 5);

        let pre = EventTriagePreFilter {
            fallback_urgent_cutoff_priority: 80,
            fallback_ignore_cutoff_priority: 20,
            max_events_per_triage: 50,
            default_priority: 0,
            event_type_priority,
        };

        let events = vec![mk_event("e1", WorldEventType::TimeUpdate)];

        let err = SessionTriageEngine::fallback_priority_split_error(&events, &pre);
        assert!(err[0].reason.starts_with("LLM失败-规则兜底("));

        let to = SessionTriageEngine::fallback_priority_split_timeout(&events, &pre);
        assert!(to[0].reason.starts_with("LLM超时-规则兜底("));
    }
}
