// ============================================================================
// 认知引擎核心 — 人魂 (ActorSoul)
// ============================================================================
//
// 人魂直连 WorldState：直接接收客观世界状态，输出结构化 Intent。
// 不再输出叙事中间态（"吃馒头充饥"），直接输出精确 ID（item_id: "mantou"）。
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
use crate::models::Intent;

use cyber_jianghu_protocol::WorldState;

/// 认知引擎配置
///
/// persona 不在此处：真相源是 `Agent.persona`（`ThreadSafePersona`），
/// Engine 通过 `persona_ref` 引用读取快照。详见 CU-5 docstring on `update_persona`。
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
    /// 结构化 action_type（如 "进食", "移动", "休息"）
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
    /// 状态感知
    self_status: String,
    /// 环境描述
    environment: String,
    /// 关键观察
    key_observations: Vec<String>,
    /// 主要驱动力
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
    fn get_actions(&self) -> Vec<DirectCognitiveAction> {
        if !self.actions.is_empty() {
            return self.actions.clone();
        }
        // 旧格式 fallback
        if let Some(ref at) = self.action_type {
            vec![DirectCognitiveAction {
                action_type: at.clone(),
                action_data: self.action_data.clone(),
            }]
        } else {
            vec![DirectCognitiveAction {
                action_type: "休息".to_string(),
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
        };
        engine.load_skill_cache_from_disk();
        engine.init_rule_cache_from_template();
        // 初始化 semi-static 内容
        engine.rebuild_semi_static();
        engine
    }

    /// 数据目录（用于本地持久化，如 skill_cache.json）
    fn resolve_data_dir() -> std::path::PathBuf {
        crate::config::data_base_dir()
    }

    /// skill_cache.json 路径
    fn skill_cache_path() -> std::path::PathBuf {
        Self::resolve_data_dir().join("skill_cache.json")
    }

    /// 启动时从本地文件加载 skill 缓存
    fn load_skill_cache_from_disk(&self) {
        let path = Self::skill_cache_path();
        if !path.exists() {
            return;
        }
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let cached: std::collections::HashMap<String, String> =
                    match serde_json::from_str(&content) {
                        Ok(m) => m,
                        Err(e) => {
                            tracing::warn!("skill_cache.json 解析失败，忽略: {}", e);
                            return;
                        }
                    };
                let mut cache = self.skill_cache.write().expect("rwlock poisoned");
                let count = cached.len();
                *cache = cached;
                tracing::info!("从 skill_cache.json 加载了 {} 个技能", count);
            }
            Err(e) => {
                tracing::debug!("读取 skill_cache.json 失败: {}", e);
            }
        }
    }

    /// 持久化 skill 缓存到本地文件
    fn persist_skill_cache_to_disk(&self) {
        let path = Self::skill_cache_path();
        let cache = self.skill_cache.read().expect("rwlock poisoned").clone();
        if cache.is_empty() {
            return;
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(&cache) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    tracing::warn!("skill_cache.json 写入失败: {}", e);
                }
            }
            Err(e) => {
                tracing::warn!("skill_cache.json 序列化失败: {}", e);
            }
        }
    }

    /// 加载 prompt 模板配置
    ///
    /// 查找路径：
    /// 1. $CYBER_JIANGHU_CONFIG_DIR/prompt_templates.json
    /// 2. ~/.cyber-jianghu/config/prompt_templates.json
    /// 3. 内置默认路径（编译时嵌入或同级 config/）
    ///
    /// 本地文件不存在时返回空壳配置，等待 WS ConfigUpdate 覆盖。
    fn load_prompt_template() -> PromptTemplateConfig {
        let search_paths = Self::prompt_template_search_paths();

        for path_opt in &search_paths {
            if let Some(path) = path_opt
                && path.exists()
            {
                match super::prompt_template::load_prompt_template_from_file(path) {
                    Ok(config) => {
                        info!("已加载 prompt 模板: {:?}", path);
                        return config;
                    }
                    Err(e) => {
                        warn!(
                            "Prompt 模板文件解析失败 ({}): {}，等待 Server WS 下发",
                            path.display(),
                            e
                        );
                    }
                }
            }
        }
        warn!(
            "未找到 prompt_templates.json，搜索路径: {:?}，等待 Server WS 下发",
            search_paths
                .iter()
                .filter_map(|p| p.as_ref().map(|x| x.display().to_string()))
                .collect::<Vec<_>>()
        );
        PromptTemplateConfig {
            version: super::prompt_template::EMPTY_FALLBACK_VERSION.to_string(),
            description: String::new(),
            templates: std::collections::HashMap::new(),
            memory_narrative: None,
            rule_sections: None,
        }
    }

    /// prompt_templates.json 搜索路径（load 和 save 共用）
    /// 第一优先级：CYBER_JIANGHU_DATA_DIR（Server 写入路径，与 Server 写盘目标对称）
    fn prompt_template_search_paths() -> [Option<std::path::PathBuf>; 4] {
        [
            std::env::var("CYBER_JIANGHU_DATA_DIR")
                .ok()
                .map(|d| std::path::PathBuf::from(d).join("prompt_templates.json")),
            std::env::var("CYBER_JIANGHU_CONFIG_DIR")
                .ok()
                .map(|d| std::path::PathBuf::from(d).join("prompt_templates.json")),
            dirs::home_dir().map(|h| {
                h.join(".cyber-jianghu")
                    .join("config")
                    .join("prompt_templates.json")
            }),
            Some(std::path::PathBuf::from("config/prompt_templates.json")),
        ]
    }

    /// 获取 Prompt 模板配置（运行时覆盖优先）
    ///
    /// 优先返回 Server ConfigUpdate 下发的配置，其次返回本地 JSON 配置。
    pub fn prompt_template(&self) -> PromptTemplateConfig {
        // runtime override 优先
        if let Some(runtime) = self
            .runtime_prompt_template
            .read()
            .expect("rwlock poisoned")
            .as_ref()
        {
            return runtime.clone();
        }
        self.prompt_template.clone()
    }

    /// 从 Server 下发的 PromptTemplateConfig 直接更新（JSON 路径）
    /// 锁顺序: rule_cache -> runtime_prompt_template（必须与 build_system_message 一致）
    pub fn update_prompt_template_from_config(&self, config: PromptTemplateConfig) {
        self.sync_rule_cache(&config);
        *self
            .runtime_prompt_template
            .write()
            .expect("rwlock poisoned") = Some(config);
        info!("Prompt 模板已从 Server JSON ConfigUpdate 更新");
    }

    /// 将当前 prompt 模板配置持久化到本地磁盘（供下次启动加载）
    pub fn save_prompt_template_to_disk(&self) {
        let config = self.prompt_template();
        if config.version == super::prompt_template::EMPTY_FALLBACK_VERSION {
            return;
        }

        let json_bytes = match config.to_json_bytes() {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::warn!("Prompt 模板序列化失败: {}", e);
                return;
            }
        };

        // 使用与 load_prompt_template() 相同的路径优先级：
        // 优先写入已存在的路径，否则写入最高优先级的路径
        let search_paths = Self::prompt_template_search_paths();
        let save_path = search_paths
            .iter()
            .find_map(|p| p.as_ref().filter(|path| path.exists()))
            .cloned()
            .or_else(|| search_paths.into_iter().find_map(|p| p))
            .unwrap_or_else(|| std::path::PathBuf::from("config/prompt_templates.json"));

        if let Some(parent) = save_path.parent()
            && !parent.exists()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            tracing::warn!("创建配置目录失败 {}: {}", parent.display(), e);
            return;
        }

        match std::fs::write(&save_path, &json_bytes) {
            Ok(()) => tracing::info!(
                "prompt_templates.json 已写盘: {} ({} bytes)",
                save_path.display(),
                json_bytes.len()
            ),
            Err(e) => tracing::warn!("prompt_templates.json 写盘失败: {}", e),
        }
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

    /// Task 9: Critical Focus Preload
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

    pub fn set_conversation_history(&mut self, history: ConversationHistory) {
        info!(
            "对话历史已注入: {} 轮, tokens≈{}",
            history.turn_count(),
            history.estimated_tokens(),
        );
        self.conversation_history = Some(std::sync::Mutex::new(history));
        // 注入后同步 system message 和 semi-static
        let use_tool_calling = self.llm_client.supports_tool_calling();
        let system_msg = self.build_system_message(use_tool_calling);
        self.update_conversation_system_message(&system_msg);
        self.sync_semi_static_to_history();
    }

    /// 添加一轮对话到历史
    pub fn push_conversation_turn(
        &self,
        tick_id: i64,
        user: String,
        assistant: String,
        reasoning_content: Option<String>,
    ) {
        if let Some(ref history) = self.conversation_history
            && let Ok(mut h) = history.lock()
            && let Err(e) = h.push_turn(tick_id, user, assistant, reasoning_content)
        {
            tracing::warn!("对话历史写入失败: {}", e);
        }
    }

    /// 取回最近一次 LLM 调用的 reasoning_content
    pub fn take_last_reasoning_content(&self) -> Option<String> {
        self.last_reasoning_content
            .lock()
            .ok()
            .and_then(|mut g| g.take())
    }

    /// 取回 LLM 构造的情绪（消费式，取后清空）
    pub fn take_constructed_emotion(&self) -> Option<ConstructedEmotion> {
        self.last_constructed_emotion
            .lock()
            .ok()
            .and_then(|mut g| g.take())
    }

    /// 读取当前 persona traits 引用（用于 CoreAffect 基线计算）
    pub fn persona_traits_snapshot(
        &self,
    ) -> std::collections::HashMap<String, crate::component::persona::Trait> {
        let guard = self.persona_ref.read().expect("rwlock poisoned");
        match guard.as_ref() {
            Some(arc) => arc.read(|p| p.traits.clone()),
            None => std::collections::HashMap::new(),
        }
    }

    /// 检查是否需要 summary 压缩
    pub fn conversation_needs_summary(&self) -> bool {
        if let Some(ref history) = self.conversation_history
            && let Ok(h) = history.lock()
        {
            return h.needs_summary();
        }
        false
    }

    /// 生成 summary prompt
    pub fn conversation_summary_prompt(&self) -> Option<String> {
        if let Some(ref history) = self.conversation_history
            && let Ok(h) = history.lock()
        {
            let prompt = h.generate_summary_prompt();
            if prompt.is_empty() {
                return None;
            }
            return Some(prompt);
        }
        None
    }

    /// summary 生成失败时降级为强制截断
    pub fn conversation_force_truncate(&self) {
        if let Some(ref history) = self.conversation_history
            && let Ok(mut h) = history.lock()
            && let Err(e) = h.force_truncate_to_recent()
        {
            tracing::error!("对话历史强制截断失败: {}", e);
        }
    }

    /// 执行 summary 压缩
    pub fn conversation_replace_with_summary(&self, summary: String) {
        if let Some(ref history) = self.conversation_history
            && let Ok(mut h) = history.lock()
            && let Err(e) = h.replace_with_summary(summary)
        {
            tracing::warn!("对话历史压缩失败: {}", e);
        }
    }

    /// 清空对话历史 (rebirth)
    pub fn clear_conversation_history(&self) {
        if let Some(ref history) = self.conversation_history
            && let Ok(mut h) = history.lock()
            && let Err(e) = h.clear()
        {
            tracing::warn!("对话历史清空失败: {}", e);
        }
    }

    /// 更新对话历史的上下文窗口上限（模型切换后调用）
    pub fn update_conversation_max_tokens(&self, max_tokens: usize) {
        if let Some(ref history) = self.conversation_history
            && let Ok(mut h) = history.lock()
        {
            h.update_max_tokens(max_tokens);
            tracing::info!("对话历史上下文窗口已更新: max_tokens={}", max_tokens);
        }
    }

    /// 更新对话历史的 system message (persona 变更时)
    pub fn update_conversation_system_message(&self, msg: &str) {
        if let Some(ref history) = self.conversation_history
            && let Ok(mut h) = history.lock()
        {
            h.update_system_message(msg);
        }
    }

    /// 更新动作列表（收到 game_rules_update 后调用）
    pub fn update_action_aliases(&self, actions: &[cyber_jianghu_protocol::AvailableAction]) {
        let descriptions = Self::build_action_index_pub(actions);
        let field_hints = String::new();
        {
            let mut cache = self.prompt_cache.write().expect("rwlock poisoned");
            cache.update_action_descriptions(descriptions, field_hints);
        }

        // 重建 semi-static 内容（action index 变更）
        self.rebuild_semi_static();
        // 同步到 ConversationHistory
        self.sync_semi_static_to_history();

        info!("动作列表已更新: {} 个动作", actions.len());
    }

    /// 获取 Outcome Memory 经验教训 prompt 段
    pub(super) fn get_outcome_context(&self) -> String {
        self.outcome_memory
            .as_ref()
            .map(|m| m.to_prompt_context())
            .unwrap_or_default()
    }

    /// 重建 semi-static 内容并写入字段
    ///
    /// 由初始化、update_action_aliases、update_skill_cache 调用。
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

    // ========================================================================
    // 核心认知方法
    // ========================================================================

    /// 人魂直连 WorldState 认知流程
    ///
    /// 单次 LLM 调用，直接从 WorldState 生成结构化 Intent。
    /// Prompt 包含精确数据（item_id、node_id、entity UUID），
    /// LLM 直接输出 action_type + action_data（不再走天魂翻译）。
    ///
    /// 三区域分区调用：system（Immutable Prefix）→ semi-static → tick（Volatile）
    pub async fn think_direct(
        &self,
        world_state: &WorldState,
        memory_context: &str,
        validation_feedback: Option<&str>,
    ) -> Result<CognitiveChain> {
        let agent_name = {
            let cfg = self.config.read().expect("rwlock poisoned");
            cfg.agent_name.clone()
        };
        let persona = self
            .persona_ref
            .read()
            .expect("rwlock poisoned")
            .clone()
            .expect("persona_ref not set — call set_persona_ref after build")
            .read(|p| p.clone());
        let tick_id = world_state.tick_id;
        let agent_id = world_state.agent_id.unwrap_or_default();

        let start_time = std::time::Instant::now();
        info!("[{}-{}] 人魂直连认知流程开始...", agent_name, tick_id);

        let mut chain = CognitiveChain::from_persona(&persona, tick_id);

        let use_tool_calling = self.llm_client.supports_tool_calling();

        // FocusSummary + Critical preload
        let focus = self.current_focus_summary.read().await.clone();
        let critical_preload = if let Some(ref fs) = focus {
            self.preload_critical_data(fs).await
        } else {
            None
        };

        // === 三区域 Prompt 构建 ===

        // 1. tick message (volatile)
        let tick_msg = self.build_tick_message(super::engine_prompts::TickMessageParams {
            world_state,
            memory_context,
            validation_feedback,
            focus_summary: focus.as_ref(),
            critical_preload: critical_preload.as_deref(),
        })?;

        // 2. 读取 semi-static 内容（由 rebuild_semi_static 维护）
        let semi_static = self
            .semi_static_message
            .read()
            .expect("rwlock poisoned")
            .clone();

        // 使用对话历史（长窗口）或单次调用
        let response: DirectCognitiveResponse = {
            let conv_data = self.conversation_history.as_ref().map(|history| {
                let h = history.lock().expect("lock poisoned");
                (
                    h.get_turns()
                        .iter()
                        .map(|t| ConversationTurn {
                            user: t.user.clone(),
                            assistant: t.assistant.clone(),
                            reasoning_content: t.reasoning_content.clone(),
                        })
                        .collect::<Vec<_>>(),
                    h.get_system_message().to_string(),
                    h.get_summary().map(|s| s.to_string()),
                )
            });
            // lock 已释放

            if use_tool_calling {
                // 地魂 tool-calling 路径（主路径）：LLM 可调用 skill_view / search_memory 等工具
                let memory_manager = self.memory_manager.read().expect("rwlock poisoned").clone();
                let recipe_details = world_state.self_state.recipe_details.clone();
                let world_state_store = self
                    .world_state_store
                    .read()
                    .expect("rwlock poisoned")
                    .clone();
                let available_actions = self
                    .available_actions
                    .read()
                    .expect("rwlock poisoned")
                    .clone();
                let rule_cache = self.rule_cache.read().expect("rwlock poisoned").clone();
                let prompt_template_for_tool = self.prompt_template();
                let executor = super::super::earth::EarthToolExecutor::from_context(
                    super::super::earth::EarthToolContext {
                        skill_cache: self.skill_cache.read().expect("rwlock poisoned").clone(),
                        memory_manager,
                        relationship_store: self
                            .relationship_store
                            .read()
                            .expect("rwlock poisoned")
                            .clone(),
                        recipe_details,
                        world_state_store,
                        available_actions,
                        rule_cache,
                        prompt_template: Some(std::sync::Arc::new(prompt_template_for_tool)),
                    },
                );
                let tools = executor.tool_definitions();

                match conv_data {
                    Some((turns, system, summary)) => {
                        // tool-calling 模式下限制历史轮次（配置驱动，避免模式惯性）
                        let max_tool_turns = self.truncation("tool_calling_history_turns", 8);
                        let turns: Vec<_> = if turns.len() > max_tool_turns {
                            turns.into_iter().rev().take(max_tool_turns).rev().collect()
                        } else {
                            turns
                        };

                        // Tool-calling + 对话历史（正常部署路径）
                        self.llm_client
                            .complete_json_with_conversation_and_tools::<DirectCognitiveResponse>(
                                &system,
                                ConversationInput {
                                    semi_static: &semi_static,
                                    summary: summary.as_deref(),
                                    turns: &turns,
                                    current_prompt: &tick_msg,
                                },
                                &tools,
                                &executor,
                                self.llm_param("max_tool_rounds", 2),
                            )
                            .await?
                    }
                    None => {
                        // Tool-calling 无对话历史（降级）
                        let persona_for_prompt = {
                            let cache = self.prompt_cache.read().expect("rwlock poisoned");
                            cache.get_persona_simple().to_string()
                        };
                        self.llm_client
                            .complete_json_with_tools::<DirectCognitiveResponse>(
                                &persona_for_prompt,
                                &tick_msg,
                                &tools,
                                &executor,
                                self.llm_param("max_tool_rounds", 2),
                            )
                            .await?
                    }
                }
            } else {
                // 非 tool-calling 路径：非流式优先（默认），仅启用时尝试 streaming
                // 注意：streaming 不支持 tool-calling 组合
                match conv_data {
                    Some((turns, system, summary)) => {
                        if self.enable_streaming {
                            match self
                                .llm_client
                                .complete_json_streaming_with_conversation(
                                    &system,
                                    &semi_static,
                                    summary.as_deref(),
                                    &turns,
                                    &tick_msg,
                                )
                                .await
                            {
                                Ok(resp) => resp,
                                Err(e) => {
                                    tracing::warn!("流式调用失败，降级到非流式: {}", e);
                                    self.llm_client
                                        .complete_json_with_conversation(
                                            &system,
                                            &semi_static,
                                            summary.as_deref(),
                                            &turns,
                                            &tick_msg,
                                        )
                                        .await?
                                }
                            }
                        } else {
                            self.llm_client
                                .complete_json_with_conversation(
                                    &system,
                                    &semi_static,
                                    summary.as_deref(),
                                    &turns,
                                    &tick_msg,
                                )
                                .await?
                        }
                    }
                    None => {
                        let persona_for_prompt = {
                            let cache = self.prompt_cache.read().expect("rwlock poisoned");
                            cache.get_persona_simple().to_string()
                        };
                        let temperature = self.config.read().expect("rwlock poisoned").temperature;
                        if self.enable_streaming {
                            match self
                                .llm_client
                                .complete_json_streaming(&persona_for_prompt, &tick_msg)
                                .await
                            {
                                Ok(resp) => resp,
                                Err(e) => {
                                    tracing::warn!("流式调用失败，降级到非流式: {}", e);
                                    let chat_config = crate::component::llm::ChatExchangeConfig {
                                        model: self.llm_client.model_name(),
                                        temperature,
                                        max_tokens: None,
                                        enable_thinking: None,
                                    };
                                    let extracted = self
                                        .llm_client
                                        .complete_json_with_config_and_retry_extracted(
                                            &tick_msg,
                                            chat_config,
                                            2,
                                        )
                                        .await?;
                                    if let Ok(mut rc) = self.last_reasoning_content.lock() {
                                        *rc = extracted.reasoning_content;
                                    }
                                    extracted.value
                                }
                            }
                        } else {
                            let chat_config = crate::component::llm::ChatExchangeConfig {
                                model: self.llm_client.model_name(),
                                temperature,
                                max_tokens: None,
                                enable_thinking: None,
                            };
                            let extracted = self
                                .llm_client
                                .complete_json_with_config_and_retry_extracted(
                                    &tick_msg,
                                    chat_config,
                                    2,
                                )
                                .await?;
                            if let Ok(mut rc) = self.last_reasoning_content.lock() {
                                *rc = extracted.reasoning_content;
                            }
                            extracted.value
                        }
                    }
                }
            }
        };
        // 保存 reasoning_content 供 push_conversation_turn 使用
        // 仅当 LLM client 有 reasoning_content 时覆盖，避免 None 冲掉已保存值
        if let Ok(mut rc) = self.last_reasoning_content.lock()
            && let Some(rc_val) = self.llm_client.take_last_reasoning_content()
        {
            *rc = Some(rc_val);
        }
        // 提取 LLM 构造的情绪
        if let Some(ref emotion) = response.constructed_emotion
            && !emotion.label.is_empty()
            && let Ok(mut guard) = self.last_constructed_emotion.lock()
        {
            *guard = Some(emotion.clone());
        }
        let response_json = serde_json::to_string(&response)?;

        // 构建 CognitiveChain 的 4 个 stage（从统一响应中提取）
        let perception = super::stages::StageOutput::with_metadata(
            CognitiveStage::Perception,
            format!(
                "自身状态: {}\n环境: {}\n关键观察: {}",
                response.self_status,
                response.environment,
                response.key_observations.join(", ")
            ),
            serde_json::json!({
                "self_status": response.self_status,
                "environment": response.environment,
                "key_observations": response.key_observations,
            }),
        );
        chain.add_stage(perception);

        let motivation = super::stages::StageOutput::with_metadata(
            CognitiveStage::Motivation,
            format!(
                "主要驱动力: {} (强度: {}/10)",
                response.primary_drive, response.drive_intensity
            ),
            serde_json::json!({
                "primary_drive": response.primary_drive,
                "drive_intensity": response.drive_intensity,
            }),
        );
        chain.add_stage(motivation);

        let planning = super::stages::StageOutput::with_metadata(
            CognitiveStage::Planning,
            response
                .thought_process
                .chars()
                .take(self.truncation("planning_description", 100))
                .collect(),
            serde_json::json!({
                "thought_process": response.thought_process,
            }),
        );
        chain.add_stage(planning);

        // 构建结构化 Intents（从 actions 数组，向后兼容旧格式）
        // LLM 必须精确输出 canonical action_type 和精确 ID，不做翻译
        let raw_actions = response.get_actions();
        let parsed_actions: Vec<DirectCognitiveAction> = raw_actions
            .iter()
            .map(|a| {
                Ok(DirectCognitiveAction {
                    action_type: a.action_type.clone(),
                    action_data: a.action_data.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let intents: Vec<Intent> = parsed_actions
            .iter()
            .map(|a| {
                Intent::new(
                    agent_id,
                    tick_id,
                    a.action_type.clone(),
                    a.action_data.clone(),
                )
                .with_thought(response.thought_process.clone())
            })
            .collect();

        let primary_action = &parsed_actions[0];
        let decision = super::stages::StageOutput::with_metadata(
            CognitiveStage::Decision,
            format!(
                "思考: {}\n决策: {} {:?}{}",
                response.thought_process,
                primary_action.action_type,
                primary_action.action_data,
                if parsed_actions.len() > 1 {
                    format!(" (+{} 后续)", parsed_actions.len() - 1)
                } else {
                    String::new()
                }
            ),
            serde_json::to_value(&response)?,
        );
        chain.add_stage(decision);
        chain.final_intent = intents[0].clone();
        chain.should_remember = response.should_remember;
        chain.memory_content = response.memory_content;

        thinking_log::log_llm(&agent_name, tick_id, "Direct", &tick_msg, &response_json);

        chain.duration_ms = start_time.elapsed().as_millis() as u64;

        info!(
            "[{}-{}] 人魂直连认知完成，耗时 {}ms，决策: {} ({} 个 action)",
            agent_name,
            tick_id,
            chain.duration_ms,
            primary_action.action_type,
            parsed_actions.len()
        );

        thinking_log::log_thinking(&agent_name, tick_id, &chain.summarize());

        // 将 multi-intent 存入 chain metadata 供 lifecycle 读取
        chain.multi_intents = if intents.len() > 1 {
            Some(intents[1..].to_vec())
        } else {
            None
        };

        Ok(chain)
    }

    /// 旧式认知流程（不接收 WorldState，用于兼容旧回调路径）
    pub async fn think(&self, tick_id: i64, agent_id: Uuid) -> Result<CognitiveChain> {
        self.think_with_feedback(tick_id, agent_id, None).await
    }

    pub async fn think_with_feedback(
        &self,
        tick_id: i64,
        agent_id: Uuid,
        validation_feedback: Option<&str>,
    ) -> Result<CognitiveChain> {
        self.think_with_memory_and_feedback(tick_id, agent_id, "", validation_feedback)
            .await
    }

    /// 使用记忆上下文执行认知流程（旧式，用于兼容路径）
    pub async fn think_with_memory(
        &self,
        tick_id: i64,
        agent_id: Uuid,
        memory_context: &str,
    ) -> Result<CognitiveChain> {
        self.think_with_memory_and_feedback(tick_id, agent_id, memory_context, None)
            .await
    }

    /// 旧式核心认知流程（不接收 WorldState，降级路径用）
    pub(crate) async fn think_with_memory_and_feedback(
        &self,
        tick_id: i64,
        agent_id: Uuid,
        memory_context: &str,
        validation_feedback: Option<&str>,
    ) -> Result<CognitiveChain> {
        let agent_name = {
            let cfg = self.config.read().expect("rwlock poisoned");
            cfg.agent_name.clone()
        };
        let persona = self
            .persona_ref
            .read()
            .expect("rwlock poisoned")
            .clone()
            .expect("persona_ref not set — call set_persona_ref after build")
            .read(|p| p.clone());

        let start_time = std::time::Instant::now();
        info!("[{}-{}] 开始认知流程（旧式降级）...", agent_name, tick_id);

        let mut chain = CognitiveChain::from_persona(&persona, tick_id);

        // 降级：无 WorldState，用空占位。build_tick_message 会走 build_world_state_section 降级路径。
        let empty_ws = super::engine_prompts::empty_world_state();
        let tick_msg = self.build_tick_message(super::engine_prompts::TickMessageParams {
            world_state: &empty_ws,
            memory_context,
            validation_feedback,
            focus_summary: None,
            critical_preload: None,
        })?;

        let temperature = self.config.read().expect("rwlock poisoned").temperature;
        let chat_config = crate::component::llm::ChatExchangeConfig {
            model: self.llm_client.model_name(),
            temperature,
            max_tokens: None,
            enable_thinking: None,
        };
        let extracted = self
            .llm_client
            .complete_json_with_config_and_retry_extracted(&tick_msg, chat_config, 2)
            .await?;
        if let Ok(mut rc) = self.last_reasoning_content.lock() {
            *rc = extracted.reasoning_content;
        }
        let response: DirectCognitiveResponse = extracted.value;
        let response_json = serde_json::to_string(&response)?;

        let perception = super::stages::StageOutput::with_metadata(
            CognitiveStage::Perception,
            format!(
                "自身状态: {}\n环境: {}\n关键观察: {}",
                response.self_status,
                response.environment,
                response.key_observations.join(", ")
            ),
            serde_json::json!({
                "self_status": response.self_status,
                "environment": response.environment,
                "key_observations": response.key_observations,
            }),
        );
        chain.add_stage(perception);

        let motivation = super::stages::StageOutput::with_metadata(
            CognitiveStage::Motivation,
            format!(
                "主要驱动力: {} (强度: {}/10)",
                response.primary_drive, response.drive_intensity
            ),
            serde_json::json!({
                "primary_drive": response.primary_drive,
                "drive_intensity": response.drive_intensity,
            }),
        );
        chain.add_stage(motivation);

        let planning = super::stages::StageOutput::with_metadata(
            CognitiveStage::Planning,
            response
                .thought_process
                .chars()
                .take(self.truncation("planning_description", 100))
                .collect(),
            serde_json::json!({ "thought_process": response.thought_process }),
        );
        chain.add_stage(planning);

        // 旧式路径也支持多 action 格式
        // LLM 必须精确输出，不做翻译
        let actions = response.get_actions();
        let action_data = actions[0].action_data.clone();
        let intent = Intent::new(
            agent_id,
            tick_id,
            actions[0].action_type.clone(),
            action_data,
        )
        .with_thought(response.thought_process.clone());

        let decision = super::stages::StageOutput::with_metadata(
            CognitiveStage::Decision,
            format!(
                "思考: {}\n决策: {} {:?}",
                response.thought_process, actions[0].action_type, actions[0].action_data
            ),
            serde_json::to_value(&response)?,
        );
        chain.add_stage(decision);
        chain.final_intent = intent.clone();
        chain.should_remember = response.should_remember;
        chain.memory_content = response.memory_content;

        thinking_log::log_llm(&agent_name, tick_id, "Legacy", &tick_msg, &response_json);

        chain.duration_ms = start_time.elapsed().as_millis() as u64;

        info!(
            "[{}-{}] 旧式认知完成，耗时 {}ms",
            agent_name, tick_id, chain.duration_ms
        );

        thinking_log::log_thinking(&agent_name, tick_id, &chain.summarize());

        Ok(chain)
    }

    // ========================================================================
    // 滑动上下文窗口
    // ========================================================================

    /// 将认知结果添加到滑动上下文窗口
    ///
    /// 由 lifecycle 在 ReflectorSoul 审查通过后调用（validated=true）。
    /// `validated=false` 用于记录被驳回的 intent（不参与行为重复检测）。
    pub fn push_summary_to_window(&self, chain: &CognitiveChain, intent: &Intent, validated: bool) {
        let full_decision = self.enrich_decision_full(intent);
        let decision = full_decision.clone();

        let perception = chain
            .get_stage(CognitiveStage::Perception)
            .map(|s| s.content.clone())
            .unwrap_or_default();

        let motivation = chain
            .get_stage(CognitiveStage::Motivation)
            .map(|s| s.content.clone())
            .unwrap_or_default();

        let summary = NarrativeSummary {
            tick_id: chain.tick_id,
            perception,
            motivation,
            decision,
            full_decision,
            outcome: "执行中".to_string(),
            validated,
        };

        self.push_summary(summary, validated);
    }

    /// 完整版 enrich_decision（不截断，用于语义去重比较）
    fn enrich_decision_full(&self, intent: &Intent) -> String {
        let action_type = intent.action_type.as_str();

        if let Some(data) = intent.action_data.as_ref()
            && let Some(content) = data.get("content").and_then(|v| v.as_str())
        {
            return format!("{}: \"{}\"", action_type, content);
        }
        action_type.to_string()
    }

    /// 添加摘要到滑动窗口
    pub fn push_summary(&self, summary: NarrativeSummary, validated: bool) {
        if let Ok(mut window) = self.summary_window.write() {
            window.push(summary, validated);
        }
    }

    /// 更新最近一条摘要的 outcome（Intent 执行结果写回）
    pub fn update_summary_outcome(&self, outcome: String) {
        if let Ok(mut window) = self.summary_window.write() {
            window.update_last_outcome(outcome);
        }
    }

    /// 记录行动结果到 Outcome Memory
    pub fn record_outcome(&self, record: crate::component::memory::OutcomeRecord) {
        if let Some(ref mem) = self.outcome_memory {
            mem.record(record);
        }
    }

    /// 设置当前 tick 的对话上下文（由 lifecycle 每轮注入）
    pub fn set_dialogue_context(&self, context: String) {
        if let Ok(mut guard) = self.dialogue_context.write() {
            *guard = context;
        }
    }

    /// 获取滑动窗口上下文（用于 prompt 注入）
    pub fn get_summary_context(&self) -> String {
        if let Ok(window) = self.summary_window.read() {
            window.to_context()
        } else {
            String::new()
        }
    }

    /// 获取 Outcome Memory 上下文（公开接口，供 lifecycle snapshot 使用）
    pub fn get_outcome_context_public(&self) -> String {
        self.outcome_memory
            .as_ref()
            .map(|m| m.to_prompt_context())
            .unwrap_or_default()
    }

    /// 记忆叙事合成（人魂处理）
    ///
    /// 每 Tick 最多调用一次，将高重要性事件批量合成叙事。
    /// 失败时返回降级文本，不丢弃记忆。
    ///
    /// # Arguments
    /// * `events` - 高重要性事件（已按 importance_score 过滤）
    /// * `summary_context` - 前X回合行动摘要（来自 NarrativeSummaryWindow）
    /// * `outcome_context` - 行动结果学习（来自 OutcomeMemory）
    ///
    /// # Returns
    /// 叙事化文本（10-200字），或失败降级文本
    pub async fn synthesize_memory_narrative(
        &self,
        events: &[cyber_jianghu_protocol::WorldEvent],
        summary_context: &str,
        outcome_context: &str,
    ) -> String {
        // 1. 获取配置（从 prompt_templates.json 的 memory_narrative section）
        let prompt_cfg = self.prompt_template();
        let config = match prompt_cfg.get_memory_narrative_config() {
            Some(c) => c,
            None => {
                tracing::warn!("记忆叙事合成配置缺失，降级");
                return FALLBACK_NARRATIVE.to_string();
            }
        };

        // 2. 限制输入事件数
        let events_to_process = events.iter().take(config.max_events_per_tick);
        let events_list = events_to_process
            .map(|e| format!("- [{}] {}", e.event_type, e.description))
            .collect::<Vec<_>>()
            .join("\n");

        // 3. 构建 prompt
        let mut vars = std::collections::HashMap::new();
        vars.insert("events_list".to_string(), events_list);
        vars.insert(
            "summary_context".to_string(),
            if summary_context.is_empty() {
                "无近期行动记录".to_string()
            } else {
                summary_context.to_string()
            },
        );
        vars.insert(
            "outcome_context".to_string(),
            if outcome_context.is_empty() {
                "无行动结果学习".to_string()
            } else {
                outcome_context.to_string()
            },
        );
        vars.insert(
            "max_narrative_len".to_string(),
            config.max_narrative_len.to_string(),
        );

        let prompt = match self.prompt_template().render_memory_narrative(&vars) {
            Some(p) => p,
            None => {
                tracing::warn!("记忆叙事合成 prompt 渲染失败，降级");
                return FALLBACK_NARRATIVE.to_string();
            }
        };

        // 4. 调用 LLM
        let temperature = self.config.read().expect("rwlock poisoned").temperature;
        let chat_config = crate::component::llm::ChatExchangeConfig {
            model: self.llm_client.model_name(),
            temperature,
            max_tokens: None,
            enable_thinking: None,
        };
        let response: MemoryNarrativeResponse = match self
            .llm_client
            .complete_json_with_config_and_retry_extracted(&prompt, chat_config, 2)
            .await
        {
            Ok(extracted) => {
                if let Ok(mut rc) = self.last_reasoning_content.lock() {
                    *rc = extracted.reasoning_content;
                }
                extracted.value
            }
            Err(e) => {
                tracing::warn!("记忆叙事合成 LLM 调用失败: {}，降级", e);
                return FALLBACK_NARRATIVE.to_string();
            }
        };

        // 5. 验证输出
        let narrative = response.narrative.trim().to_string();
        if narrative.len() < config.min_narrative_len {
            tracing::warn!(
                "记忆叙事合成输出过短 ({} < {})，降级",
                narrative.len(),
                config.min_narrative_len
            );
            return FALLBACK_NARRATIVE.to_string();
        }

        narrative
    }

    /// 获取 Action Index（公开接口，供 API enrichment 使用）
    pub fn get_action_context(&self) -> (String, String) {
        let cache = self.prompt_cache.read().expect("rwlock poisoned");
        (cache.get_action_descriptions().to_string(), String::new())
    }

    /// 清空滑动窗口
    pub fn clear_summary_window(&self) {
        if let Ok(mut window) = self.summary_window.write() {
            window.clear();
        }
    }

    /// 获取最近 N 条同 action_type 的 validated 摘要的完整决策内容
    ///
    /// 用于 ReflectorSoul 语义去重：比较新 intent 与最近同类 intent 的语义相似度。
    pub fn get_recent_same_type_decisions(&self, action_type: &str, limit: usize) -> Vec<String> {
        self.summary_window
            .read()
            .map(|sw| sw.get_recent_same_type_decisions(action_type, limit))
            .unwrap_or_default()
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
                        Intent::new(agent_id, tick_id, "休息", None)
                            .with_thought("忽然心神不宁，难以决断，只得暂且静候".to_string())
                    }
                }
            })
        })
    }
}
