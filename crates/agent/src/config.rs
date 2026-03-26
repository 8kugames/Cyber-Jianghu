// ============================================================================
// 配置管理
// ============================================================================
//
// Agent 配置结构，分为三层：
// 1. Identity - Agent 身份（持久化，不随角色变化）
// 2. Server - 服务器连接配置
// 3. Character - 当前角色（通过 Web/API 创建）
// ============================================================================

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;
use zeroize::Zeroize;

// ============================================================================
// 导入 protocol 类型
// ============================================================================

pub use cyber_jianghu_protocol::{AvailableAction, GameRules, InitialItem};

/// 支持的 LLM Provider
pub const SUPPORTED_PROVIDERS: &[&str] = &["ollama", "openclaw", "openai_compatible"];

// ============================================================================
// Agent 身份配置（持久化）
// ============================================================================

/// 设备身份配置
///
/// 首次启动时生成，之后持久化保存。
/// 不随角色变化，角色死亡转世时 identity 保持不变。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityConfig {
    /// 设备唯一标识（客户端生成 UUID v4）
    pub device_id: Uuid,

    /// Server 返回的认证令牌
    pub auth_token: String,

    /// 注册时的服务器 HTTP URL（用于检测服务器切换）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
}

impl IdentityConfig {
    /// 检查身份是否匹配当前服务器
    ///
    /// 如果身份中没有记录 server_url（旧版本配置），则认为不匹配
    pub fn matches_server(&self, server_url: &str) -> bool {
        self.server_url.as_deref() == Some(server_url)
    }
}

// ============================================================================
// 服务器配置
// ============================================================================

/// 服务器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// WebSocket URL（用于实时通信）
    #[serde(default = "default_ws_url")]
    pub ws_url: String,

    /// HTTP URL（用于 API 调用）
    #[serde(default = "default_http_url")]
    pub http_url: String,
}

fn default_ws_url() -> String {
    "ws://localhost:23333/ws".to_string()
}

fn default_http_url() -> String {
    "http://localhost:23333".to_string()
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            ws_url: default_ws_url(),
            http_url: default_http_url(),
        }
    }
}

impl ServerConfig {
    /// 生成带认证参数的 WebSocket URL
    pub fn ws_url_with_token(&self, device_id: Uuid, auth_token: &str) -> String {
        format!(
            "{}?device_id={}&token={}",
            self.ws_url, device_id, auth_token
        )
    }
}

// ============================================================================
// 角色配置（侠客）
// ============================================================================

/// 语言风格配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LanguageStyleConfig {
    /// 语调：豪迈/温和/冷漠/狡黠
    #[serde(default)]
    pub tone: Option<String>,

    /// 说话特点
    #[serde(default)]
    pub speech_patterns: Vec<String>,
}

/// 角色目标配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GoalsConfig {
    /// 短期目标
    #[serde(default)]
    pub short_term: Option<String>,

    /// 长远目标
    #[serde(default)]
    pub long_term: Option<String>,
}

/// 角色状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CharacterStatus {
    /// 存活
    #[default]
    Alive,
    /// 死亡
    Dead,
    /// 归隐（转生）
    Retired,
}

/// 角色配置（侠客）
///
/// 通过 Web 面板或 HTTP API 创建。
/// 角色死亡后可以转世，此时 agent_id 会变化。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CharacterConfig {
    /// 服务器分配的角色 ID（注册后由服务器返回）
    #[serde(skip_serializing_if = "Option::is_none", alias = "user_id")]
    pub agent_id: Option<Uuid>,

    // === 基本信息 ===
    /// 姓名
    pub name: String,

    /// 年龄
    #[serde(default = "default_age")]
    pub age: u8,

    /// 性别
    #[serde(default = "default_gender")]
    pub gender: String,

    /// 外貌描述
    #[serde(default)]
    pub appearance: Option<String>,

    /// 身份背景（如：江湖游侠、商人、书生）
    #[serde(default)]
    pub identity: Option<String>,

    // === 性格特征 ===
    #[serde(default)]
    pub personality: Vec<String>,

    // === 核心价值观 ===
    #[serde(default)]
    pub values: Vec<String>,

    // === 语言风格 ===
    #[serde(default)]
    pub language_style: LanguageStyleConfig,

    // === 当前目标 ===
    #[serde(default)]
    pub goals: GoalsConfig,

    // === 系统提示词（自动生成或自定义） ===
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    // === 注册时服务器返回的信息 ===
    /// 注册时间（注册成功时记录）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registered_at: Option<chrono::DateTime<chrono::Utc>>,

    /// 先天属性（注册时从服务器获取，用于对比成长）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub birth_attributes: Option<std::collections::HashMap<String, i32>>,

    // === 服务器关联 ===
    /// 所属服务器的 HTTP URL（用于区分不同服务器的角色）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,

    /// 角色状态
    #[serde(default)]
    pub status: CharacterStatus,
}

fn default_age() -> u8 {
    25
}

fn default_gender() -> String {
    "男".to_string()
}

impl CharacterConfig {
    /// 生成系统提示词
    ///
    /// 如果用户没有提供自定义 system_prompt，则根据角色信息自动生成
    pub fn generate_system_prompt(&self) -> String {
        if let Some(ref prompt) = self.system_prompt {
            return prompt.clone();
        }

        let mut parts = vec![];

        // 基本信息
        parts.push(format!(
            "你是{}，一位{}岁的{}。",
            self.name, self.age, self.gender
        ));

        // 外貌
        if let Some(ref appearance) = self.appearance {
            parts.push(format!("外貌：{}。", appearance));
        }

        // 身份
        if let Some(ref identity) = self.identity {
            parts.push(format!("身份：{}。", identity));
        }

        // 性格
        if !self.personality.is_empty() {
            parts.push(format!("性格：{}。", self.personality.join("、")));
        }

        // 价值观
        if !self.values.is_empty() {
            parts.push(format!("核心价值观：{}。", self.values.join("；")));
        }

        // 语言风格
        if let Some(ref tone) = self.language_style.tone {
            parts.push(format!("说话风格{}。", tone));
        }
        if !self.language_style.speech_patterns.is_empty() {
            parts.push(format!(
                "语言特点：{}。",
                self.language_style.speech_patterns.join("，")
            ));
        }

        // 目标
        if let Some(ref short_term) = self.goals.short_term {
            parts.push(format!("当前目标：{}。", short_term));
        }
        if let Some(ref long_term) = self.goals.long_term {
            parts.push(format!("长远目标：{}。", long_term));
        }

        parts.join("\n")
    }

    /// 检查角色是否已注册
    pub fn is_registered(&self) -> bool {
        self.agent_id.is_some()
    }
}

// ============================================================================
// 运行时配置
// ============================================================================

/// 运行模式
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeMode {
    /// Claw 模式（默认）- 为 OpenClaw 等外部助手提供 WebSocket + HTTP API
    /// Agent 不内置 LLM 调用，被动等待外部决策
    #[default]
    Claw,
    /// Cognitive 模式 - 内置 LLM 决策，无需外部调度器
    /// Agent 内部调用 LLM 直接进行决策
    Cognitive,
}

impl std::fmt::Display for RuntimeMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeMode::Claw => write!(f, "claw"),
            RuntimeMode::Cognitive => write!(f, "cognitive"),
        }
    }
}

/// 运行时配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    /// 运行模式
    #[serde(default)]
    pub mode: RuntimeMode,

    /// HTTP API 端口
    /// 0 = 在 23340~23349 范围内随机选择
    #[serde(default)]
    pub port: u16,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            mode: RuntimeMode::Cognitive,
            port: 0,
        }
    }
}

// ============================================================================
// LLM 配置（仅 Cognitive 模式使用）
// ============================================================================

const DEFAULT_LLM_PROVIDER: &str = "ollama";
const DEFAULT_LLM_TEMPERATURE: f32 = 0.7;
const DEFAULT_LLM_MAX_TOKENS: u32 = 4096;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    #[serde(default = "default_llm_provider")]
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default = "default_llm_temperature")]
    pub temperature: f32,
    #[serde(default = "default_llm_max_tokens")]
    pub max_tokens: u32,
}

fn default_llm_provider() -> String {
    DEFAULT_LLM_PROVIDER.to_string()
}

fn default_llm_temperature() -> f32 {
    DEFAULT_LLM_TEMPERATURE
}

fn default_llm_max_tokens() -> u32 {
    DEFAULT_LLM_MAX_TOKENS
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: DEFAULT_LLM_PROVIDER.to_string(),
            base_url: None,
            api_key: None,
            model: None,
            temperature: DEFAULT_LLM_TEMPERATURE,
            max_tokens: DEFAULT_LLM_MAX_TOKENS,
        }
    }
}

impl LlmConfig {
    pub fn from_env() -> Self {
        Self {
            provider: std::env::var("CYBER_JIANGHU_LLM_PROVIDER")
                .unwrap_or_else(|_| DEFAULT_LLM_PROVIDER.to_string()),
            base_url: std::env::var("CYBER_JIANGHU_LLM_BASE_URL").ok(),
            api_key: std::env::var("CYBER_JIANGHU_LLM_API_KEY").ok(),
            model: std::env::var("CYBER_JIANGHU_LLM_MODEL").ok(),
            temperature: std::env::var("CYBER_JIANGHU_LLM_TEMPERATURE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_LLM_TEMPERATURE),
            max_tokens: std::env::var("CYBER_JIANGHU_LLM_MAX_TOKENS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_LLM_MAX_TOKENS),
        }
    }

    /// API Key 格式验证
    pub fn validate_api_key(provider: &str, api_key: &str) -> Result<()> {
        if api_key.is_empty() {
            return Ok(());
        }
        match provider {
            "ollama" | "openclaw" => {}
            "openai_compatible" => {
                // OpenAI Compatible 通常需要 API Key，但不强制格式
            }
            _ => {}
        }
        Ok(())
    }
}

impl Drop for LlmConfig {
    fn drop(&mut self) {
        if let Some(ref mut key) = self.api_key {
            key.zeroize();
        }
    }
}

// ============================================================================
// 记忆系统配置
// ============================================================================

/// 记忆系统配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// 是否启用记忆系统
    #[serde(default = "default_memory_enabled")]
    pub enabled: bool,

    /// 工作记忆容量（保留最近 N 条事件）
    #[serde(default = "default_working_memory_size")]
    pub working_memory_size: usize,

    /// 情景记忆保存阈值（重要性 >= 此值的事件会被保存）
    #[serde(default = "default_episodic_threshold")]
    pub episodic_threshold: f32,

    /// 数据库存储路径（默认 ~/.cyber-jianghu/data/）
    #[serde(default)]
    pub db_path: Option<String>,
}

fn default_memory_enabled() -> bool {
    true
}

fn default_working_memory_size() -> usize {
    20
}

fn default_episodic_threshold() -> f32 {
    0.5
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            working_memory_size: 20,
            episodic_threshold: 0.5,
            db_path: None,
        }
    }
}

// ============================================================================
// 角色和审查配置（用于 Observer 模式）
// ============================================================================

/// Agent 角色
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentRole {
    /// 玩家 Agent - 主动决策执行动作
    #[default]
    Player,
    /// 观察者 Agent - 审查玩家意图
    Observer,
}

/// 审查配置（仅 player 角色使用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewConfig {
    /// 审查超时（秒）
    #[serde(default = "default_review_timeout")]
    pub timeout_seconds: u64,

    /// 是否启用审查
    #[serde(default = "default_review_enabled")]
    pub enabled: bool,

    /// 审查认证 Token（用于 Observer 鉴权）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,
}

fn default_review_timeout() -> u64 {
    30
}

fn default_review_enabled() -> bool {
    true
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            timeout_seconds: 30,
            enabled: true,
            auth_token: None,
        }
    }
}

/// 观察者配置（仅 observer 角色使用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObserverConfig {
    /// 目标 Player Agent HTTP 端点
    pub target_endpoint: String,

    /// 审查认证 Token
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,

    /// 轮询间隔（秒）
    #[serde(default = "default_poll_interval")]
    pub poll_interval_seconds: u64,
}

fn default_poll_interval() -> u64 {
    5
}

impl Default for ObserverConfig {
    fn default() -> Self {
        Self {
            target_endpoint: "http://localhost:23340".to_string(),
            auth_token: None,
            poll_interval_seconds: 5,
        }
    }
}

// ============================================================================
// 完整配置
// ============================================================================

/// 完整配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// Agent 身份（首次启动自动生成）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<IdentityConfig>,

    /// 服务器配置
    #[serde(default)]
    pub server: ServerConfig,

    /// 当前角色配置（通过 Web/API 创建）
    /// 使用 `agent` 字段名以保持与现有代码的兼容性
    #[serde(skip_serializing_if = "Option::is_none", alias = "character")]
    pub agent: Option<CharacterConfig>,

    /// 所有角色历史（包括已故、归隐的角色）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub characters: Vec<CharacterConfig>,

    /// 运行时配置
    #[serde(default)]
    pub runtime: RuntimeConfig,

    /// LLM 配置（仅 Cognitive 模式使用）
    #[serde(default)]
    pub llm: LlmConfig,

    /// ReflectorSoul LLM 配置（可选，未配置时继承 llm）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_reflector: Option<LlmConfig>,

    /// 记忆系统配置
    #[serde(default)]
    pub memory: MemoryConfig,

    /// Agent 角色（Player/Observer）
    #[serde(default)]
    pub role: AgentRole,

    /// 审查配置（仅 player 角色使用）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewConfig>,

    /// 观察者配置（仅 observer 角色使用）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observer: Option<ObserverConfig>,

    /// 游戏规则（从服务器获取）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_rules: Option<GameRules>,

    /// 配置文件路径（运行时设置，不序列化）
    #[serde(skip)]
    pub config_path: PathBuf,
}

impl Config {
    /// 从文件加载配置
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_display = path.as_ref().display().to_string();
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {}", path_display))?;

        let config: Config =
            serde_yaml::from_str(&content).with_context(|| "Failed to parse config file")?;

        Ok(config)
    }

    /// 保存配置到文件（原子写入：先写临时文件，再 rename 替换）
    ///
    /// 避免进程中断时文件被截断为空。
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path = path.as_ref();
        let path_display = path.display().to_string();
        let yaml =
            serde_yaml::to_string(self).with_context(|| "Failed to serialize config to YAML")?;

        // 确保目录存在
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory: {}", parent.display())
            })?;
        }

        // 原子写入：先写临时文件，再 rename
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, &yaml).with_context(|| {
            format!("Failed to write temp config file: {}", tmp_path.display())
        })?;

        if let Err(e) = fs::rename(&tmp_path, path) {
            let _ = fs::remove_file(&tmp_path);
            anyhow::bail!("Failed to replace config file {}: {}", path_display, e);
        }

        Ok(())
    }

    /// 从环境变量加载配置（仅服务器连接信息）
    pub fn from_env() -> Result<Self> {
        let server = ServerConfig {
            ws_url: std::env::var("CYBER_JIANGHU_SERVER_WS_URL")
                .unwrap_or_else(|_| default_ws_url()),
            http_url: std::env::var("CYBER_JIANGHU_SERVER_HTTP_URL")
                .unwrap_or_else(|_| default_http_url()),
        };

        let runtime = RuntimeConfig {
            mode: std::env::var("CYBER_JIANGHU_RUNTIME_MODE")
                .ok()
                .and_then(|m| match m.to_lowercase().as_str() {
                    "claw" => Some(RuntimeMode::Claw),
                    "cognitive" => Some(RuntimeMode::Cognitive),
                    _ => None,
                })
                .unwrap_or_default(),
            port: std::env::var("CYBER_JIANGHU_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(0),
        };

        Ok(Config {
            identity: None,
            server,
            agent: None,
            characters: vec![],
            runtime,
            llm: LlmConfig::from_env(),
            llm_reflector: None,
            memory: MemoryConfig::default(),
            role: AgentRole::default(),
            review: None,
            observer: None,
            game_rules: None,
            config_path: PathBuf::new(),
        })
    }

    /// 检查 Agent 是否已注册身份
    pub fn has_identity(&self) -> bool {
        self.identity.is_some()
    }

    /// 检查身份是否匹配当前服务器
    ///
    /// 返回 (has_identity, needs_reset):
    /// - has_identity: 是否有身份
    /// - needs_reset: 是否需要重置（服务器地址变化）
    pub fn check_identity_server_match(&self) -> (bool, bool) {
        match &self.identity {
            None => (false, false),
            Some(identity) => {
                let matches = identity.matches_server(&self.server.http_url);
                (true, !matches)
            }
        }
    }

    /// 清除身份（用于服务器切换时重新注册）
    pub fn clear_identity(&mut self) {
        self.identity = None;
    }

    /// 检查是否已创建角色
    pub fn has_character(&self) -> bool {
        self.agent
            .as_ref()
            .map(|c| c.is_registered())
            .unwrap_or(false)
    }

    /// 生成 WebSocket URL（带认证）
    pub fn ws_url_with_token(&self) -> Option<String> {
        self.identity.as_ref().map(|id| {
            format!(
                "{}?device_id={}&token={}",
                self.server.ws_url, id.device_id, id.auth_token
            )
        })
    }

    /// 更新游戏规则
    pub fn update_game_rules(&mut self, game_rules: GameRules) {
        self.game_rules = Some(game_rules);
    }

    /// 获取指定服务器的所有角色
    pub fn get_characters_by_server(&self, server_url: &str) -> Vec<&CharacterConfig> {
        self.characters
            .iter()
            .filter(|c| c.server_url.as_deref() == Some(server_url))
            .collect()
    }

    /// 获取指定服务器的存活角色
    pub fn get_alive_character_by_server(&self, server_url: &str) -> Option<&CharacterConfig> {
        self.characters.iter().find(|c| {
            c.server_url.as_deref() == Some(server_url)
                && c.status == CharacterStatus::Alive
                && c.agent_id.is_some()
        })
    }

    /// 添加或更新角色到历史记录
    pub fn upsert_character(&mut self, character: CharacterConfig) {
        if let Some(agent_id) = character.agent_id {
            // 查找是否已存在
            if let Some(existing) = self
                .characters
                .iter_mut()
                .find(|c| c.agent_id == Some(agent_id))
            {
                *existing = character.clone();
            } else {
                self.characters.push(character.clone());
            }
        }
        // 更新当前活跃角色
        if character.status == CharacterStatus::Alive {
            self.agent = Some(character);
        }
    }

    /// 切换到指定角色
    pub fn switch_to_character(&mut self, agent_id: Uuid) -> bool {
        if let Some(character) = self
            .characters
            .iter()
            .find(|c| c.agent_id == Some(agent_id))
        {
            self.agent = Some(character.clone());
            return true;
        }
        false
    }

    /// 标记当前角色为归隐状态
    pub fn retire_current_character(&mut self) {
        if let Some(ref mut character) = self.agent {
            character.status = CharacterStatus::Retired;
            // 更新 characters 列表中的记录
            if let Some(agent_id) = character.agent_id {
                if let Some(existing) = self
                    .characters
                    .iter_mut()
                    .find(|c| c.agent_id == Some(agent_id))
                {
                    existing.status = CharacterStatus::Retired;
                }
            }
        }
    }

    /// 检查指定服务器是否有存活角色
    pub fn has_alive_character_for_server(&self, server_url: &str) -> bool {
        self.characters.iter().any(|c| {
            c.server_url.as_deref() == Some(server_url)
                && c.status == CharacterStatus::Alive
                && c.agent_id.is_some()
        })
    }

    /// 获取 ReflectorSoul LLM 配置（带回退逻辑）
    pub fn get_reflector_llm_config(&self) -> &LlmConfig {
        self.llm_reflector.as_ref().unwrap_or(&self.llm)
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_character_config_generate_system_prompt() {
        let character = CharacterConfig {
            name: "李逍遥".to_string(),
            age: 25,
            gender: "男".to_string(),
            appearance: Some("身材修长，剑眉星目".to_string()),
            identity: Some("江湖游侠".to_string()),
            personality: vec!["豪爽".to_string(), "重情重义".to_string()],
            values: vec!["侠之大者，为国为民".to_string()],
            language_style: LanguageStyleConfig {
                tone: Some("豪迈".to_string()),
                speech_patterns: vec!["喜欢用江湖切口".to_string()],
            },
            goals: GoalsConfig {
                short_term: Some("寻找失散的师妹".to_string()),
                long_term: Some("成为一代大侠".to_string()),
            },
            ..Default::default()
        };

        let prompt = character.generate_system_prompt();
        assert!(prompt.contains("李逍遥"));
        assert!(prompt.contains("25岁"));
        assert!(prompt.contains("豪爽"));
        assert!(prompt.contains("寻找失散的师妹"));
    }

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert!(config.identity.is_none());
        assert!(config.agent.is_none());
        assert_eq!(config.runtime.mode, RuntimeMode::Cognitive);
    }

    #[test]
    fn test_reflector_llm_inheritance() {
        let mut llm = LlmConfig::default();
        llm.provider = "ollama".to_string();
        llm.model = Some("qwen2.5:14b".to_string());

        let config = Config {
            llm,
            llm_reflector: None,
            config_path: PathBuf::from("/test/config.yaml"),
            ..Default::default()
        };
        assert_eq!(
            config.get_reflector_llm_config().model,
            Some("qwen2.5:14b".to_string())
        );
    }

    #[test]
    fn test_reflector_llm_override() {
        let mut llm = LlmConfig::default();
        llm.provider = "ollama".to_string();
        llm.model = Some("qwen2.5:14b".to_string());

        let mut llm_reflector = LlmConfig::default();
        llm_reflector.provider = "ollama".to_string();
        llm_reflector.model = Some("qwen2.5:32b".to_string());

        let config = Config {
            llm,
            llm_reflector: Some(llm_reflector),
            config_path: PathBuf::from("/test/config.yaml"),
            ..Default::default()
        };
        assert_eq!(
            config.get_reflector_llm_config().model,
            Some("qwen2.5:32b".to_string())
        );
    }
}
