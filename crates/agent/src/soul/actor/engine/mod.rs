// ============================================================================
// 认知引擎核心 — 人魂 (ActorSoul)
// ============================================================================
//
// 人魂直连 WorldState：直接接收客观世界状态，输出结构化 Intent。
// 不再输出叙事中间态（"吃馒头充饥"），直接输出精确 ID（item_id: "馒头" 或 "馒头[a65df604]"）。
// 天魂翻译步骤已消除。
//
// 地魂 tool-calling 集成：当 LLM 支持 tool calling 时，认知流程可调用
// skill_view / search_memory / recall_archived 工具按需获取精确数据。

use anyhow::Result;
use serde_json;
use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;

use super::chain::CognitiveChain;
use super::prompt_cache::PromptCache;
use super::prompt_template::PromptTemplateConfig;
use super::stages::CognitiveStage;
use super::summary_window::{NarrativeSummary, NarrativeSummaryWindow};
use crate::component::llm::conversation::ConversationHistory;
use crate::component::llm::{ConversationInput, ConversationTurn, LlmClient, LlmClientExt};
use crate::component::persona::ThreadSafePersona;
use crate::component::social::RelationshipStore;
use crate::infra::api::cognitive_context::load_available_actions_from_file;
use crate::infra::api::thinking_log;
use crate::infra::api::trace;
use crate::models::Intent;

use cyber_jianghu_protocol::WorldState;

mod conversation;
mod narrative;
mod prompt_template;
mod skill_cache;
mod think;

/// 认知引擎配置
///
/// persona 不在此处：真相源是 `Agent.persona`（`ThreadSafePersona`），
/// Engine 通过 `persona_ref` 引用读取快照。详见 `update_persona` 的 docstring。
#[derive(Clone, Debug)]
pub struct CognitiveEngineConfig {
    /// Agent 名称
    pub agent_name: String,
    /// 温度参数
    pub temperature: f32,
    /// 每阶段最大 token 数
    pub max_tokens_per_stage: u32,
}

/// 单个结构化 action
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct DirectCognitiveAction {
    /// 结构化 action_type（如 "用", "移动", "休整"）
    pub action_type: String,
    /// 结构化 action_data（精确 ID）
    pub action_data: Option<serde_json::Value>,
}

/// 记忆叙事合成响应
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct MemoryNarrativeResponse {
    narrative: String,
}

/// 失败降级文本（用户指定，一字不差）
pub(crate) const FALLBACK_NARRATIVE: &str = "你一阵恍惚，似乎遗漏了一些重要的记忆。";

/// LLM 构造的具体情绪
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
pub struct ConstructedEmotion {
    pub label: String,
    pub reasoning: String,
    pub intensity: f32,
}

/// 人魂统一认知响应（单次 LLM 调用，直连 WorldState，输出结构化 Intent）
///
/// 支持两种 LLM 输出格式（向后兼容）：
/// - 新格式: `actions: [{action_type, action_data}, ...]` — 1-3 个 sequential actions
/// - 旧格式: `action_type + action_data` — 单个 action（自动转换为 actions 数组）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct DirectCognitiveResponse {
    #[serde(default)]
    self_status: serde_json::Value,
    #[serde(default)]
    environment: serde_json::Value,
    /// 关键观察（模型偶发省略；仅用于 trace 展示，容忍缺失）
    #[serde(default)]
    key_observations: Vec<String>,
    /// 主要驱动力（模型偶发省略；仅用于 trace 展示，容忍缺失）
    #[serde(default = "crate::soul::actor::stages::default_primary_drive")]
    primary_drive: String,
    /// 驱动力强度 (1-10)
    drive_intensity: u8,
    /// 思考过程
    thought_process: String,
    /// 多 action 格式（新）
    #[serde(default)]
    actions: Vec<DirectCognitiveAction>,
    /// 单 action 格式（旧，向后兼容）
    #[serde(default)]
    action_type: Option<String>,
    /// 单 action_data 格式（旧，向后兼容）
    #[serde(default)]
    action_data: Option<serde_json::Value>,
    /// 是否应写入记忆（人魂判断）
    #[serde(default)]
    should_remember: Option<bool>,
    /// 要写入记忆的内容（人魂判断，should_remember=true时必填）
    #[serde(default)]
    memory_content: Option<String>,
    /// LLM 构造的具体情绪
    #[serde(default)]
    constructed_emotion: Option<ConstructedEmotion>,
}

impl DirectCognitiveResponse {
    /// 统一获取 actions 列表
    ///
    /// 优先使用 `actions` 字段（新格式），fallback 到 `action_type` + `action_data`（旧格式）。
    /// 返回前做动作名规范化：LLM 输出的语义/英文别名（如 idle/进食/给予/采集）
    /// 映射为 canonical 键并注入缺省字段，未命中别名的原样通过（由后续审查/Server 兑底）。
    fn get_actions(&self) -> Vec<DirectCognitiveAction> {
        let normalize = |mut a: DirectCognitiveAction| {
            a.action_type =
                cyber_jianghu_protocol::normalize_action_type(&a.action_type, &mut a.action_data);
            a
        };
        if !self.actions.is_empty() {
            return self.actions.iter().cloned().map(normalize).collect();
        }
        // 旧格式 fallback
        if let Some(ref at) = self.action_type {
            vec![normalize(DirectCognitiveAction {
                action_type: at.clone(),
                action_data: self.action_data.clone(),
            })]
        } else {
            vec![DirectCognitiveAction {
                action_type: "休整".to_string(),
                action_data: None,
            }]
        }
    }
}

/// 认知引擎（人魂直连 WorldState）
///
/// 单次 LLM 调用，直接从 WorldState 生成结构化 Intent。
/// Prompt 中包含精确的 item_id、node_id、entity UUID，
/// LLM 直接输出可执行的 Intent（不再走天魂翻译）。
///
/// 【Prompt 缓存优化】
/// 使用 PromptCache 缓存 persona 和 actions，减少重复内容。
///
/// 【滑动上下文窗口】
/// 使用 NarrativeSummaryWindow 保留最近 N 轮的行动轨迹摘要，
/// 帮助 LLM 理解连续决策的上下文。
pub struct CognitiveEngine {
    llm_client: Arc<dyn LlmClient>,
    pub(super) config: std::sync::RwLock<CognitiveEngineConfig>,
    /// 流式 LLM 调用（默认启用，非流式作为降级路径）
    enable_streaming: bool,
    /// Prompt 缓存（分层缓存优化）
    pub(super) prompt_cache: std::sync::RwLock<PromptCache>,
    /// 滑动上下文窗口（保留最近 N 轮摘要）
    summary_window: std::sync::RwLock<NarrativeSummaryWindow>,
    /// 当前 tick 的对话上下文（由 lifecycle 注入，build_tick_message 读取）
    pub(super) dialogue_context: std::sync::RwLock<String>,
    /// 对话历史（长窗口，SQLite 持久化）
    conversation_history: Option<std::sync::Mutex<ConversationHistory>>,
    /// Prompt 模板配置（从 YAML 加载，启动时 fail-fast）
    pub(super) prompt_template: PromptTemplateConfig,
    /// 运行时 Prompt 模板配置（来自 Server ConfigUpdate，覆盖 prompt_template）
    /// Server 下发时非空，启动时为 None
    runtime_prompt_template: std::sync::RwLock<Option<PromptTemplateConfig>>,
    /// 行动结果记忆（Hermes 模式）
    pub(super) outcome_memory: Option<crate::component::memory::OutcomeMemory>,
    /// SKILL.md body 缓存（skill_id → body content），避免每 tick 重复 IO
    pub(super) skill_cache: std::sync::RwLock<std::collections::HashMap<String, String>>,
    /// 记忆管理器引用（用于地魂 search_memory / recall_archived）
    pub(super) memory_manager: std::sync::RwLock<
        Option<std::sync::Arc<tokio::sync::RwLock<crate::component::memory::MemoryManager>>>,
    >,
    /// 关系存储（用于地魂 get_relationship / list_relationships / record_social_event）
    pub(super) relationship_store: std::sync::RwLock<Option<RelationshipStore>>,
    /// WorldState 本地落存（供地魂 query_world / get_action_detail 工具使用）
    pub(super) world_state_store:
        std::sync::RwLock<Option<Arc<crate::component::state_store::WorldStateStore>>>,
    /// 可用动作列表（供地魂 get_action_detail 工具使用）
    pub(super) available_actions:
        std::sync::RwLock<Vec<cyber_jianghu_protocol::types::entities::AvailableAction>>,
    /// 当前 tick 的 FocusSummary（由 lifecycle 写入，供 lean prompt 读取）
    pub(super) current_focus_summary:
        Arc<tokio::sync::RwLock<Option<crate::component::attention::FocusSummary>>>,
    /// 最近一次 LLM 调用的 reasoning_content（DeepSeek 等需要回传多轮对话）
    last_reasoning_content: std::sync::Mutex<Option<String>>,
    /// 最近一次 LLM 构造的情绪（供 lifecycle 回写 persona）
    last_constructed_emotion: std::sync::Mutex<Option<ConstructedEmotion>>,
    /// Semi-static prompt 内容（action index + skill index），配置更新时重建
    semi_static_message: std::sync::RwLock<String>,
    /// Agent 人设引用（真相源在 Agent, Engine 通过 Arc 读取快照构建 prompt）
    persona_ref: std::sync::RwLock<Option<std::sync::Arc<ThreadSafePersona>>>,
    /// 规则缓存（EarthSoul query_rules tool 按需检索）
    pub(super) rule_cache: std::sync::RwLock<Option<crate::component::rule_cache::RuleCache>>,
    /// 上轮行动执行结果摘要（由 lifecycle 写入，供 build_tick_message 注入人魂推理上下文）
    last_tick_action_summary: std::sync::RwLock<String>,
    /// 上轮天魂驳回记录（内容 + 发生 tick；由 soul_cycle 写入）。
    /// 天魂驳回不产生 ExecutionResult，不写入 last_tick_action_summary，
    /// 若不留痕下一回合 LLM 对失败零记忆 → 重复同样臆造。TTL 由读取端控制
    last_tick_rejection: std::sync::RwLock<(String, i64)>,
}

impl CognitiveEngine {
    /// 创建新的认知引擎
    pub fn new(
        llm_client: Arc<dyn LlmClient>,
        config: CognitiveEngineConfig,
        persona: &ThreadSafePersona,
    ) -> Self {
        let (persona_desc, persona_for_cache) =
            persona.read(|p| (p.generate_description(), p.clone()));
        let (action_descriptions, action_field_hints) = Self::load_actions_list();
        let prompt_cache = PromptCache::new(
            persona_desc,
            action_descriptions,
            action_field_hints,
            &persona_for_cache,
        );

        let prompt_template = Self::load_prompt_template();

        let engine = Self {
            llm_client,
            config: std::sync::RwLock::new(config),
            enable_streaming: true,
            prompt_cache: std::sync::RwLock::new(prompt_cache),
            summary_window: std::sync::RwLock::new(NarrativeSummaryWindow::new(
                crate::config::DEFAULT_NARRATIVE_WINDOW_SIZE,
            )),
            dialogue_context: std::sync::RwLock::new(String::new()),
            conversation_history: None,
            prompt_template,
            runtime_prompt_template: std::sync::RwLock::new(None),
            outcome_memory: None,
            skill_cache: std::sync::RwLock::new(std::collections::HashMap::new()),
            memory_manager: std::sync::RwLock::new(None),
            relationship_store: std::sync::RwLock::new(None),
            world_state_store: std::sync::RwLock::new(None),
            available_actions: std::sync::RwLock::new(Vec::new()),
            current_focus_summary: Arc::new(tokio::sync::RwLock::new(None)),
            last_reasoning_content: std::sync::Mutex::new(None),
            last_constructed_emotion: std::sync::Mutex::new(None),
            semi_static_message: std::sync::RwLock::new(String::new()),
            persona_ref: std::sync::RwLock::new(Some(std::sync::Arc::new(persona.clone()))),
            rule_cache: std::sync::RwLock::new(None),
            last_tick_action_summary: std::sync::RwLock::new(String::new()),
            last_tick_rejection: std::sync::RwLock::new((String::new(), 0)),
        };
        engine.load_skill_cache_from_disk();
        engine.init_rule_cache_from_template();
        // 初始化 semi-static 内容
        engine.rebuild_semi_static();
        engine
    }

    /// 从 PromptTemplateConfig 同步 RuleCache：有配置则重建，无则清除
    fn sync_rule_cache(&self, config: &PromptTemplateConfig) {
        match config.rule_sections {
            Some(ref rs) if rs.enabled && !rs.categories.is_empty() => {
                let cache = crate::component::rule_cache::RuleCache::new(rs);
                *self.rule_cache.write().expect("rwlock poisoned") = Some(cache);
                info!("RuleCache 已重建，{} 个分类", rs.categories.len());
            }
            _ => {
                *self.rule_cache.write().expect("rwlock poisoned") = None;
            }
        }
    }

    /// 从本地 prompt_template 初始化 RuleCache（冷启动路径）
    fn init_rule_cache_from_template(&self) {
        self.sync_rule_cache(&self.prompt_template);
    }

    /// 设置 NarrativeSummaryWindow 窗口大小
    pub fn set_narrative_window_size(&self, size: usize) {
        let mut window = self.summary_window.write().expect("rwlock poisoned");
        *window = NarrativeSummaryWindow::new(size);
    }

    /// 空转 tick 占位摘要：不调用 LLM，仅在叙事窗口记录本次跳过的风味文本。
    /// full_decision 不匹配任何 action_type 前缀，不会参与语义去重。
    pub fn record_idle_summary(&self, tick_id: i64, flavor: &str) {
        let summary = NarrativeSummary {
            tick_id,
            perception: "无显著变化".to_string(),
            motivation: "维持当前状态".to_string(),
            decision: flavor.to_string(),
            full_decision: flavor.to_string(),
            outcome: "无".to_string(),
            validated: true,
        };
        self.summary_window
            .write()
            .expect("rwlock poisoned")
            .push(summary, true);
    }

    /// 更新技能缓存（接收 ConfigUpdate 后调用）
    ///
    /// - update_type == "full": 全量替换，先清空再插入
    /// - update_type == "incremental": 增量更新，插入新版 + 移除已删除的
    ///
    /// 更新后自动持久化到本地文件。
    pub fn update_skill_cache(
        &self,
        skills: Vec<cyber_jianghu_protocol::types::SkillContent>,
        removed_items: Vec<String>,
    ) {
        let mut cache = self.skill_cache.write().expect("rwlock poisoned");
        let skills_count = skills.len();
        let removed_count = removed_items.len();

        // 处理增量更新：移除已删除的技能
        for skill_id in &removed_items {
            cache.remove(skill_id);
            tracing::debug!("Removed skill from cache: {}", skill_id);
        }

        // 插入/更新技能
        for skill in skills {
            cache.insert(skill.skill_id.clone(), skill.body);
        }

        tracing::debug!(
            "Updated skill cache: +{} skills, -{} removed, total {}",
            skills_count,
            removed_count,
            cache.len()
        );

        // drop lock before persisting and rebuilding
        drop(cache);
        self.persist_skill_cache_to_disk();
        // 重建 semi-static 内容（skill index 变更）
        self.rebuild_semi_static();
        self.sync_semi_static_to_history();
    }

    /// 设置是否启用流式 LLM 调用
    pub fn set_enable_streaming(&mut self, enable: bool) {
        self.enable_streaming = enable;
    }

    /// 使用自定义窗口大小创建认知引擎
    pub fn with_window_size(
        llm_client: Arc<dyn LlmClient>,
        config: CognitiveEngineConfig,
        window_size: usize,
        persona: &ThreadSafePersona,
    ) -> Self {
        let (persona_desc, persona_for_cache) =
            persona.read(|p| (p.generate_description(), p.clone()));
        let (action_descriptions, action_field_hints) = Self::load_actions_list();
        let prompt_cache = PromptCache::new(
            persona_desc,
            action_descriptions,
            action_field_hints,
            &persona_for_cache,
        );

        let prompt_template = Self::load_prompt_template();

        let engine = Self {
            llm_client,
            config: std::sync::RwLock::new(config),
            enable_streaming: true,
            prompt_cache: std::sync::RwLock::new(prompt_cache),
            summary_window: std::sync::RwLock::new(NarrativeSummaryWindow::new(window_size)),
            dialogue_context: std::sync::RwLock::new(String::new()),
            conversation_history: None,
            prompt_template,
            runtime_prompt_template: std::sync::RwLock::new(None),
            outcome_memory: None,
            skill_cache: std::sync::RwLock::new(std::collections::HashMap::new()),
            memory_manager: std::sync::RwLock::new(None),
            relationship_store: std::sync::RwLock::new(None),
            world_state_store: std::sync::RwLock::new(None),
            available_actions: std::sync::RwLock::new(Vec::new()),
            current_focus_summary: Arc::new(tokio::sync::RwLock::new(None)),
            last_reasoning_content: std::sync::Mutex::new(None),
            last_constructed_emotion: std::sync::Mutex::new(None),
            semi_static_message: std::sync::RwLock::new(String::new()),
            persona_ref: std::sync::RwLock::new(Some(std::sync::Arc::new(persona.clone()))),
            rule_cache: std::sync::RwLock::new(None),
            last_tick_action_summary: std::sync::RwLock::new(String::new()),
            last_tick_rejection: std::sync::RwLock::new((String::new(), 0)),
        };
        engine.load_skill_cache_from_disk();
        engine.init_rule_cache_from_template();
        // 初始化 semi-static 内容
        engine.rebuild_semi_static();
        engine
    }

    /// 获取截断长度配置（数据驱动替代 .take(N) 魔法数字）
    pub fn truncation(&self, key: &str, default: usize) -> usize {
        self.prompt_template()
            .truncation("actor_direct", key, default)
    }

    /// 获取 LLM 调用参数配置（数据驱动替代硬编码参数）
    pub(super) fn llm_param(&self, key: &str, default: usize) -> usize {
        self.prompt_template()
            .llm_param("actor_direct", key, default)
    }

    /// 加载动作列表（用于缓存）
    fn load_actions_list() -> (String, String) {
        let available_actions = load_available_actions_from_file();
        let descriptions = Self::build_action_index_pub(&available_actions);
        let field_hints = String::new();
        (descriptions, field_hints)
    }

    /// 使用默认配置创建
    /// 更新 Agent 名称（注册新角色后调用）
    pub fn update_agent_name(&self, new_name: &str) {
        let mut config = self.config.write().expect("rwlock poisoned");
        config.agent_name = new_name.to_string();
        info!("认知引擎 agent_name 已更新: {}", new_name);
    }

    /// 设置 Outcome Memory（由 builder 在构建后注入）
    pub fn set_outcome_memory(&mut self, mem: crate::component::memory::OutcomeMemory) {
        self.outcome_memory = Some(mem);
    }

    /// 设置 Memory Manager（由 builder 在构建后注入）
    pub fn set_memory_manager(
        &self,
        manager: std::sync::Arc<tokio::sync::RwLock<crate::component::memory::MemoryManager>>,
    ) {
        let mut mem_guard = self.memory_manager.write().expect("rwlock poisoned");
        *mem_guard = Some(manager);
    }

    /// 设置对话历史（由 lifecycle 在注册后注入）
    pub fn set_relationship_store(&self, store: RelationshipStore) {
        let mut guard = self.relationship_store.write().expect("rwlock poisoned");
        *guard = Some(store);
    }

    /// 设置 WorldStateStore（由 lifecycle 注入，供地魂 query_world 工具使用）
    pub fn set_world_state_store(
        &self,
        store: Arc<crate::component::state_store::WorldStateStore>,
    ) {
        let mut guard = self.world_state_store.write().expect("rwlock poisoned");
        *guard = Some(store);
    }

    /// 设置可用动作列表（由 lifecycle 注入，供地魂 get_action_detail 工具使用）
    pub fn set_available_actions(
        &self,
        actions: Vec<cyber_jianghu_protocol::types::entities::AvailableAction>,
    ) {
        let mut guard = self.available_actions.write().expect("rwlock poisoned");
        *guard = actions;
    }

    /// 更新当前 tick 的 FocusSummary（由 lifecycle 在每 tick 写入）
    pub async fn set_current_focus_summary(
        &self,
        summary: Option<crate::component::attention::FocusSummary>,
    ) {
        *self.current_focus_summary.write().await = summary;
    }

    /// Critical Focus Preload
    ///
    /// 当 FocusSummary 包含 Critical 紧急项时，预加载相关 WorldState 分区数据。
    /// 在 think_direct() 内部调用，异步读取 WorldStateStore。
    async fn preload_critical_data(
        &self,
        focus_summary: &crate::component::attention::FocusSummary,
    ) -> Option<String> {
        // Clone Arc 在 lock 作用域内，避免跨 await 持有 std::sync::RwLockReadGuard
        let store = {
            let store_guard = self.world_state_store.read().expect("rwlock poisoned");
            store_guard.as_ref().cloned()?
        };

        let has_critical = focus_summary
            .items
            .iter()
            .any(|i| i.change.urgency == crate::component::delta_engine::Urgency::Critical);
        if !has_critical {
            return None;
        }

        use std::collections::HashSet;
        let categories: HashSet<_> = focus_summary
            .items
            .iter()
            .filter(|i| i.change.urgency == crate::component::delta_engine::Urgency::Critical)
            .map(|i| i.change.category.clone())
            .collect();

        let mut preloaded = String::from("\n### 紧急状态预加载\n");
        for cat in &categories {
            let section = match cat {
                crate::component::delta_engine::ChangeCategory::Survival => "state",
                crate::component::delta_engine::ChangeCategory::Social => "entities",
                crate::component::delta_engine::ChangeCategory::Inventory => "inventory",
                crate::component::delta_engine::ChangeCategory::Environment => "environment",
                crate::component::delta_engine::ChangeCategory::Location => "environment",
            };
            let data =
                super::super::earth::state_tool::execute_query_world(section, None, &store).await;
            if data["success"].as_bool().unwrap_or(false)
                && let Ok(pretty) = serde_json::to_string_pretty(&data)
            {
                preloaded.push_str(&pretty);
                preloaded.push('\n');
            }
        }
        Some(preloaded)
    }

    /// 重建 semi-static 内容并写入字段
    ///
    /// 由初始化、update_action_index、update_skill_cache 调用。
    fn rebuild_semi_static(&self) {
        let msg = self.build_semi_static_message();
        let mut guard = self.semi_static_message.write().expect("rwlock poisoned");
        *guard = msg;
    }

    /// 同步 semi-static 内容到 ConversationHistory
    fn sync_semi_static_to_history(&self) {
        let msg = self
            .semi_static_message
            .read()
            .expect("rwlock poisoned")
            .clone();
        if let Some(ref history) = self.conversation_history
            && let Ok(mut h) = history.lock()
        {
            h.set_semi_static_message(msg);
        }
    }

    /// 更新 persona 情绪标签（由 soul cycle 回写）
    pub fn update_persona_emotion(&self, emotion: String) {
        let guard = self.persona_ref.read().expect("rwlock poisoned");
        if let Some(ref arc) = *guard {
            arc.write(|p| p.update_emotion(emotion));
        }
    }

    /// 应用特质变化到 persona（由 ConstructedEmotion 回写调用）
    pub fn apply_persona_trait_change(
        &self,
        trait_name: &str,
        delta: i16,
        reason: String,
        tick_id: i64,
    ) {
        let guard = self.persona_ref.read().expect("rwlock poisoned");
        if let Some(ref arc) = *guard {
            arc.write(|p| p.apply_trait_change(trait_name, delta, reason, tick_id));
        }
    }

    /// 更新 Agent 人设（rebirth 后调用）
    ///
    /// 行为契约:
    /// - 改: agent_name
    /// - 保留: persona.traits, persona.current_state（历史事件积累的状态）
    /// - 刷新: prompt_cache（下一 tick 重建 persona_desc 和 persona_summary）
    ///
    /// 实施位置: 此方法当前在 CognitiveEngine 内，persona 真相源在 Agent。
    /// 调用方必须在调用前更新 agent.persona.name + base_description。
    pub fn update_persona(&self, name: &str, _system_prompt: &str) {
        self.update_agent_name(name);
        if let Some(ref arc) = *self.persona_ref.read().expect("rwlock poisoned") {
            self.invalidate_persona_cache(arc);
        }

        // 重建 system message（persona 变更）
        let use_tool_calling = self.llm_client.supports_tool_calling();
        let system_msg = self.build_system_message(use_tool_calling);
        self.update_conversation_system_message(&system_msg);

        info!("认知引擎人设已更新: name={}", name);
    }

    /// 每 tick 末尾调用：刷新 prompt cache 让下一 tick LLM 看到最新 traits
    pub fn invalidate_persona_cache(&self, persona: &ThreadSafePersona) {
        let (new_desc, persona_clone) = persona.read(|p| (p.generate_description(), p.clone()));
        let mut cache = self.prompt_cache.write().expect("rwlock poisoned");
        cache.invalidate_persona(new_desc, &persona_clone);
    }

    /// 设置 Agent 人设引用（Agent 构造后由 builder 调用一次）
    pub fn set_persona_ref(&self, persona: std::sync::Arc<ThreadSafePersona>) {
        let mut guard = self.persona_ref.write().expect("rwlock poisoned");
        *guard = Some(persona);
    }
}

// ============================================================================
// 创建 DecisionCallback 的便捷方法
// ============================================================================

impl CognitiveEngine {
    /// 创建决策回调（兼容旧接口，不接收 WorldState）
    pub fn create_decision_callback(self) -> crate::runtime::DecisionCallback {
        let engine = Arc::new(self);
        Arc::new(move |tick_id: i64, agent_id: uuid::Uuid| {
            let engine = engine.clone();
            Box::pin(async move {
                match engine.think(tick_id, agent_id).await {
                    Ok(chain) => chain.final_intent,
                    Err(e) => {
                        tracing::error!("多阶段认知失败: {}", e);
                        Intent::new(agent_id, tick_id, "休整", None)
                            .with_thought("忽然心神不宁，难以决断，只得暂且静候".to_string())
                    }
                }
            })
        })
    }
}
