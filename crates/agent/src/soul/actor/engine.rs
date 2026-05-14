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
use super::translation::{ActionAliasMap, EntityTranslationRegistry, FieldAliasMap};
use crate::component::llm::conversation::ConversationHistory;
use crate::component::llm::{ConversationInput, ConversationTurn, LlmClient, LlmClientExt};
use crate::component::persona::DynamicPersona;
use crate::component::social::RelationshipStore;
use crate::infra::api::cognitive_context::load_available_actions_from_file;
use crate::infra::api::thinking_log;
use crate::models::Intent;

use cyber_jianghu_protocol::WorldState;

/// 认知引擎配置
#[derive(Clone, Debug)]
pub struct CognitiveEngineConfig {
    /// Agent 名称
    pub agent_name: String,
    /// Agent 动态人设
    pub persona: DynamicPersona,
    /// 温度参数
    pub temperature: f32,
    /// 每阶段最大 token 数
    pub max_tokens_per_stage: u32,
}

impl Default for CognitiveEngineConfig {
    fn default() -> Self {
        let agent_id = uuid::Uuid::new_v4();
        let persona = DynamicPersona::new(agent_id, "无名侠客", "你是一名行走在江湖中的侠客。");

        Self {
            agent_name: "无名侠客".to_string(),
            persona,
            temperature: 0.7,
            max_tokens_per_stage: 1024,
        }
    }
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
    config: std::sync::RwLock<CognitiveEngineConfig>,
    /// 流式 LLM 调用（默认启用，非流式作为降级路径）
    enable_streaming: bool,
    /// Prompt 缓存（分层缓存优化）
    pub(super) prompt_cache: std::sync::RwLock<PromptCache>,
    /// 滑动上下文窗口（保留最近 N 轮摘要）
    summary_window: std::sync::RwLock<NarrativeSummaryWindow>,
    /// 对话历史（长窗口，SQLite 持久化）
    conversation_history: Option<std::sync::Mutex<ConversationHistory>>,
    /// Prompt 模板配置（从 YAML 加载，启动时 fail-fast）
    pub(super) prompt_template: PromptTemplateConfig,
    /// 运行时 Prompt 模板配置（来自 Server ConfigUpdate，覆盖 prompt_template）
    /// Server 下发时非空，启动时为 None
    runtime_prompt_template: std::sync::RwLock<Option<PromptTemplateConfig>>,
    /// 行动结果记忆（Hermes 模式）
    pub(super) outcome_memory: Option<crate::component::memory::OutcomeMemory>,
    /// action_type 别名映射（中文/别名 → 英文 canonical）
    pub(super) action_alias_map: std::sync::RwLock<ActionAliasMap>,
    /// action_data 字段别名映射（中文/别名 → 英文 canonical）
    pub(super) field_alias_map: std::sync::RwLock<FieldAliasMap>,
    /// SKILL.md body 缓存（skill_id → body content），避免每 tick 重复 IO
    pub(super) skill_cache: std::sync::RwLock<std::collections::HashMap<String, String>>,
    /// 记忆管理器引用（用于地魂 search_memory / recall_archived）
    pub(super) memory_manager: std::sync::RwLock<
        Option<std::sync::Arc<tokio::sync::RwLock<crate::component::memory::MemoryManager>>>,
    >,
    /// 关系存储（用于地魂 get_relationship / list_relationships / record_social_event）
    pub(super) relationship_store: std::sync::RwLock<Option<RelationshipStore>>,
}

impl CognitiveEngine {
    /// 创建新的认知引擎
    pub fn new(llm_client: Arc<dyn LlmClient>, config: CognitiveEngineConfig) -> Self {
        let persona_desc = config.persona.generate_description();
        let (action_descriptions, action_field_hints, alias_map, field_map) =
            Self::load_actions_list();
        let prompt_cache = PromptCache::new(
            persona_desc,
            action_descriptions,
            action_field_hints,
            &config.persona,
        );

        let prompt_template = Self::load_prompt_template();

        let engine = Self {
            llm_client,
            config: std::sync::RwLock::new(config),
            enable_streaming: true,
            prompt_cache: std::sync::RwLock::new(prompt_cache),
            summary_window: std::sync::RwLock::new(NarrativeSummaryWindow::new(3)),
            conversation_history: None,
            prompt_template,
            runtime_prompt_template: std::sync::RwLock::new(None),
            outcome_memory: None,
            action_alias_map: std::sync::RwLock::new(alias_map),
            field_alias_map: std::sync::RwLock::new(field_map),
            skill_cache: std::sync::RwLock::new(std::collections::HashMap::new()),
            memory_manager: std::sync::RwLock::new(None),
            relationship_store: std::sync::RwLock::new(None),
        };
        engine.load_skill_cache_from_disk();
        engine
    }

    /// 设置 NarrativeSummaryWindow 窗口大小
    pub fn set_narrative_window_size(&self, size: usize) {
        let mut window = self.summary_window.write().unwrap();
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
        let mut cache = self.skill_cache.write().unwrap();
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

        // drop lock before persisting
        drop(cache);
        self.persist_skill_cache_to_disk();
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
    ) -> Self {
        let persona_desc = config.persona.generate_description();
        let (action_descriptions, action_field_hints, alias_map, field_map) =
            Self::load_actions_list();
        let prompt_cache = PromptCache::new(
            persona_desc,
            action_descriptions,
            action_field_hints,
            &config.persona,
        );

        let prompt_template = Self::load_prompt_template();

        let engine = Self {
            llm_client,
            config: std::sync::RwLock::new(config),
            enable_streaming: true,
            prompt_cache: std::sync::RwLock::new(prompt_cache),
            summary_window: std::sync::RwLock::new(NarrativeSummaryWindow::new(window_size)),
            conversation_history: None,
            prompt_template,
            runtime_prompt_template: std::sync::RwLock::new(None),
            outcome_memory: None,
            action_alias_map: std::sync::RwLock::new(alias_map),
            field_alias_map: std::sync::RwLock::new(field_map),
            skill_cache: std::sync::RwLock::new(std::collections::HashMap::new()),
            memory_manager: std::sync::RwLock::new(None),
            relationship_store: std::sync::RwLock::new(None),
        };
        engine.load_skill_cache_from_disk();
        engine
    }

    /// 数据目录（用于本地持久化，如 skill_cache.json）
    fn resolve_data_dir() -> std::path::PathBuf {
        std::env::var("CYBER_JIANGHU_DATA_DIR")
            .ok()
            .map(std::path::PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".cyber-jianghu").join("data")))
            .unwrap_or_else(|| std::path::PathBuf::from("./data"))
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
                let mut cache = self.skill_cache.write().unwrap();
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
        let cache = self.skill_cache.read().unwrap().clone();
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
        if let Some(runtime) = self.runtime_prompt_template.read().unwrap().as_ref() {
            return runtime.clone();
        }
        self.prompt_template.clone()
    }

    /// 从 Server 下发的 PromptTemplateConfig 直接更新（JSON 路径）
    pub fn update_prompt_template_from_config(&self, config: PromptTemplateConfig) {
        let mut guard = self.runtime_prompt_template.write().unwrap();
        *guard = Some(config);
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
    fn llm_param(&self, key: &str, default: usize) -> usize {
        self.prompt_template()
            .llm_param("actor_direct", key, default)
    }

    /// 加载动作列表（用于缓存 + 别名映射）
    fn load_actions_list() -> (String, String, ActionAliasMap, FieldAliasMap) {
        let available_actions = load_available_actions_from_file();
        let descriptions = Self::build_action_descriptions(&available_actions);
        let field_hints = Self::build_action_field_hints(&available_actions);
        let alias_map = ActionAliasMap::from_actions(&available_actions);
        let field_map = FieldAliasMap::from_actions(&available_actions);
        (descriptions, field_hints, alias_map, field_map)
    }

    /// 使用默认配置创建
    pub fn with_defaults(llm_client: Arc<dyn LlmClient>) -> Self {
        Self::new(llm_client, CognitiveEngineConfig::default())
    }

    /// 更新 Agent 名称（注册新角色后调用）
    pub fn update_agent_name(&self, new_name: &str) {
        let mut config = self.config.write().unwrap();
        config.agent_name = new_name.to_string();
        config.persona.name = new_name.to_string();
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
        let mut mem_guard = self.memory_manager.write().unwrap();
        *mem_guard = Some(manager);
    }

    /// 设置对话历史（由 lifecycle 在注册后注入）
    pub fn set_relationship_store(&self, store: RelationshipStore) {
        let mut guard = self.relationship_store.write().unwrap();
        *guard = Some(store);
    }

    pub fn set_conversation_history(&mut self, history: ConversationHistory) {
        info!(
            "对话历史已注入: {} 轮, tokens≈{}",
            history.turn_count(),
            history.estimated_tokens(),
        );
        self.conversation_history = Some(std::sync::Mutex::new(history));
    }

    /// 添加一轮对话到历史
    pub fn push_conversation_turn(&self, tick_id: i64, user: String, assistant: String) {
        if let Some(ref history) = self.conversation_history
            && let Ok(mut h) = history.lock()
            && let Err(e) = h.push_turn(tick_id, user, assistant)
        {
            tracing::warn!("对话历史写入失败: {}", e);
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

    /// 更新对话历史的 system message (persona 变更时)
    pub fn update_conversation_system_message(&self, msg: &str) {
        if let Some(ref history) = self.conversation_history
            && let Ok(mut h) = history.lock()
        {
            h.update_system_message(msg);
        }
    }

    /// 更新动作别名映射（收到 game_rules_update 后调用）
    ///
    /// 热更新 alias map 和 field alias map，无需重建引擎。
    pub fn update_action_aliases(&self, actions: &[cyber_jianghu_protocol::AvailableAction]) {
        let new_alias_map = ActionAliasMap::from_actions(actions);
        let new_field_map = FieldAliasMap::from_actions(actions);

        {
            let mut alias_guard = self.action_alias_map.write().unwrap();
            *alias_guard = new_alias_map;
        }
        {
            let mut field_guard = self.field_alias_map.write().unwrap();
            *field_guard = new_field_map;
        }

        // 同时更新 prompt cache 中的动作描述
        let descriptions = Self::build_action_descriptions(actions);
        let field_hints = Self::build_action_field_hints(actions);
        {
            let mut cache = self.prompt_cache.write().unwrap();
            cache.update_action_descriptions(descriptions, field_hints);
        }

        info!("动作别名映射已更新: {} 个动作", actions.len());
    }

    /// 获取 Outcome Memory 经验教训 prompt 段
    pub(super) fn get_outcome_context(&self) -> String {
        self.outcome_memory
            .as_ref()
            .map(|m| m.to_prompt_context())
            .unwrap_or_default()
    }

    /// 更新 Agent 人设（rebirth 后调用）
    pub fn update_persona(&self, name: &str, system_prompt: &str) {
        let mut config = self.config.write().unwrap();
        config.agent_name = name.to_string();
        config.persona.name = name.to_string();
        config.persona.base_description = system_prompt.to_string();

        let new_desc = config.persona.generate_description();
        let mut cache = self.prompt_cache.write().unwrap();
        cache.invalidate_persona(new_desc, &config.persona);

        info!(
            "认知引擎人设已更新: name={}, prompt_len={}",
            name,
            system_prompt.len()
        );
    }

    // ========================================================================
    // 核心认知方法
    // ========================================================================

    /// 人魂直连 WorldState 认知流程
    ///
    /// 单次 LLM 调用，直接从 WorldState 生成结构化 Intent。
    /// Prompt 包含精确数据（item_id、node_id、entity UUID），
    /// LLM 直接输出 action_type + action_data（不再走天魂翻译）。
    pub async fn think_direct(
        &self,
        world_state: &WorldState,
        memory_context: &str,
        validation_feedback: Option<&str>,
    ) -> Result<CognitiveChain> {
        let (agent_name, persona) = {
            let cfg = self.config.read().unwrap();
            (cfg.agent_name.clone(), cfg.persona.clone())
        };
        let tick_id = world_state.tick_id;
        let agent_id = world_state.agent_id.unwrap_or_default();

        let start_time = std::time::Instant::now();
        info!("[{}-{}] 人魂直连认知流程开始...", agent_name, tick_id);

        let mut chain = CognitiveChain::from_persona(&persona, tick_id);

        let persona_for_prompt = {
            let mut cache = self.prompt_cache.write().unwrap();
            cache.get_persona_simple().to_string()
        };

        let use_tool_calling = self.llm_client.supports_tool_calling();

        let prompt = self.build_direct_prompt(
            world_state,
            memory_context,
            validation_feedback,
            &persona_for_prompt,
            &agent_name,
            use_tool_calling,
        )?;

        // 使用对话历史（长窗口）或单次调用
        let response: DirectCognitiveResponse = {
            let conv_data = self.conversation_history.as_ref().map(|history| {
                let h = history.lock().unwrap();
                (
                    h.get_turns()
                        .iter()
                        .map(|t| ConversationTurn {
                            user: t.user.clone(),
                            assistant: t.assistant.clone(),
                        })
                        .collect::<Vec<_>>(),
                    h.get_system_message().to_string(),
                    h.get_summary().map(|s| s.to_string()),
                )
            });
            // lock 已释放

            if use_tool_calling {
                // 地魂 tool-calling 路径（主路径）：LLM 可调用 skill_view / search_memory 等工具
                let memory_manager = self.memory_manager.read().unwrap().clone();
                let recipe_details = world_state.self_state.recipe_details.clone();
                let executor = super::super::earth::EarthToolExecutor::from_context(
                    super::super::earth::EarthToolContext {
                        skill_cache: self.skill_cache.read().unwrap().clone(),
                        memory_manager,
                        relationship_store: self.relationship_store.read().unwrap().clone(),
                        recipe_details,
                    },
                );
                let tools = super::super::earth::EarthToolExecutor::tool_definitions();

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
                                    summary: summary.as_deref(),
                                    turns: &turns,
                                    current_prompt: &prompt,
                                },
                                &tools,
                                &executor,
                                self.llm_param("max_tool_rounds", 3),
                            )
                            .await?
                    }
                    None => {
                        // Tool-calling 无对话历史（降级）
                        self.llm_client
                            .complete_json_with_tools::<DirectCognitiveResponse>(
                                &persona_for_prompt,
                                &prompt,
                                &tools,
                                &executor,
                                self.llm_param("max_tool_rounds", 3),
                            )
                            .await?
                    }
                }
            } else {
                // 非 tool-calling 路径：streaming 优先，失败降级非流式
                // 注意：streaming 不支持 tool-calling 组合
                match conv_data {
                    Some((turns, system, summary)) => {
                        if self.enable_streaming {
                            match self
                                .llm_client
                                .complete_json_streaming_with_conversation(
                                    &system,
                                    summary.as_deref(),
                                    &turns,
                                    &prompt,
                                )
                                .await
                            {
                                Ok(resp) => resp,
                                Err(e) => {
                                    tracing::warn!("流式调用失败，降级到非流式: {}", e);
                                    self.llm_client
                                        .complete_json_with_conversation(
                                            &system,
                                            summary.as_deref(),
                                            &turns,
                                            &prompt,
                                        )
                                        .await?
                                }
                            }
                        } else {
                            self.llm_client
                                .complete_json_with_conversation(
                                    &system,
                                    summary.as_deref(),
                                    &turns,
                                    &prompt,
                                )
                                .await?
                        }
                    }
                    None => {
                        if self.enable_streaming {
                            match self
                                .llm_client
                                .complete_json_streaming(&persona_for_prompt, &prompt)
                                .await
                            {
                                Ok(resp) => resp,
                                Err(e) => {
                                    tracing::warn!("流式调用失败，降级到非流式: {}", e);
                                    self.llm_client.complete_json(&prompt).await?
                                }
                            }
                        } else {
                            self.llm_client.complete_json(&prompt).await?
                        }
                    }
                }
            }
        };
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
        // 翻译硬边界：中文/别名 → canonical 中文，ReflectorSoul 只看到 canonical
        let entity_registry = EntityTranslationRegistry::from_world_state(world_state);
        let actions = response.get_actions();
        let translated_actions: Vec<DirectCognitiveAction> = actions
            .iter()
            .map(|a| {
                let action_type = self
                    .action_alias_map
                    .read()
                    .unwrap()
                    .translate(&a.action_type)
                    .ok_or_else(|| anyhow::anyhow!("未识别的动作类型: {}", a.action_type))?;
                let mut action_data = a.action_data.clone();
                if let Some(ref mut data) = action_data {
                    self.field_alias_map
                        .read()
                        .unwrap()
                        .translate_data(&action_type, data);
                    // entity-alias 翻译：所有已注册字段的值 (target_location, item_id, target_agent_id)
                    entity_registry.translate(data);
                }
                Ok(DirectCognitiveAction {
                    action_type,
                    action_data,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let intents: Vec<Intent> = translated_actions
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

        let primary_action = &translated_actions[0];
        let decision = super::stages::StageOutput::with_metadata(
            CognitiveStage::Decision,
            format!(
                "思考: {}\n决策: {} {:?}{}",
                response.thought_process,
                primary_action.action_type,
                primary_action.action_data,
                if translated_actions.len() > 1 {
                    format!(" (+{} 后续)", translated_actions.len() - 1)
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

        thinking_log::log_llm(&agent_name, tick_id, "Direct", &prompt, &response_json);

        chain.duration_ms = start_time.elapsed().as_millis() as u64;

        self.push_summary_to_window(&chain, &intents[0]);

        info!(
            "[{}-{}] 人魂直连认知完成，耗时 {}ms，决策: {} ({} 个 action)",
            agent_name,
            tick_id,
            chain.duration_ms,
            primary_action.action_type,
            translated_actions.len()
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
        let (agent_name, persona) = {
            let cfg = self.config.read().unwrap();
            (cfg.agent_name.clone(), cfg.persona.clone())
        };

        let start_time = std::time::Instant::now();
        info!("[{}-{}] 开始认知流程（旧式降级）...", agent_name, tick_id);

        let mut chain = CognitiveChain::from_persona(&persona, tick_id);

        let persona_for_prompt = {
            let mut cache = self.prompt_cache.write().unwrap();
            cache.get_persona_simple().to_string()
        };

        let prompt = self.build_legacy_prompt(
            tick_id,
            memory_context,
            validation_feedback,
            &persona_for_prompt,
            &agent_name,
        )?;

        let response: DirectCognitiveResponse = self.llm_client.complete_json(&prompt).await?;
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
        // 翻译硬边界：中文/别名 → 英文 canonical
        let actions = response.get_actions();
        let translated_type = self
            .action_alias_map
            .read()
            .unwrap()
            .translate(&actions[0].action_type)
            .unwrap_or_else(|| actions[0].action_type.clone());
        let mut action_data = actions[0].action_data.clone();
        if let Some(ref mut data) = action_data {
            self.field_alias_map
                .read()
                .unwrap()
                .translate_data(&translated_type, data);
        }
        let intent = Intent::new(agent_id, tick_id, translated_type, action_data)
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

        thinking_log::log_llm(&agent_name, tick_id, "Legacy", &prompt, &response_json);

        chain.duration_ms = start_time.elapsed().as_millis() as u64;

        self.push_summary_to_window(&chain, &intent);

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
    fn push_summary_to_window(&self, chain: &CognitiveChain, intent: &Intent) {
        let decision = intent.action_type.as_str().to_string();

        let perception = chain
            .get_stage(CognitiveStage::Perception)
            .map(|s| {
                s.content
                    .chars()
                    .take(self.truncation("summary_window", 50))
                    .collect()
            })
            .unwrap_or_default();

        let motivation = chain
            .get_stage(CognitiveStage::Motivation)
            .map(|s| {
                s.content
                    .chars()
                    .take(self.truncation("summary_window", 50))
                    .collect()
            })
            .unwrap_or_default();

        let summary = NarrativeSummary {
            tick_id: chain.tick_id,
            perception,
            motivation,
            decision,
            outcome: "执行中".to_string(),
        };

        self.push_summary(summary);
    }

    /// 添加摘要到滑动窗口
    pub fn push_summary(&self, summary: NarrativeSummary) {
        if let Ok(mut window) = self.summary_window.write() {
            window.push(summary);
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
        let response: MemoryNarrativeResponse = match self.llm_client.complete_json(&prompt).await {
            Ok(r) => r,
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

        // 6. 截断超长输出
        if narrative.len() > config.max_narrative_len {
            return narrative.chars().take(config.max_narrative_len).collect();
        }

        narrative
    }

    /// 获取 action descriptions 和 field hints（公开接口）
    pub fn get_action_context(&self) -> (String, String) {
        let cache = self.prompt_cache.read().unwrap();
        (
            cache.get_action_descriptions().to_string(),
            cache.get_action_field_hints().to_string(),
        )
    }

    /// 获取详细滑动窗口上下文（用于调试）
    #[allow(dead_code)]
    pub fn get_detailed_summary_context(&self) -> String {
        if let Ok(window) = self.summary_window.read() {
            window.to_detailed_context()
        } else {
            String::new()
        }
    }

    /// 清空滑动窗口
    pub fn clear_summary_window(&self) {
        if let Ok(mut window) = self.summary_window.write() {
            window.clear();
        }
    }

    /// 获取窗口大小
    #[allow(dead_code)]
    pub fn summary_window_size(&self) -> usize {
        if let Ok(window) = self.summary_window.read() {
            window.len()
        } else {
            0
        }
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
