// ============================================================================
// 配置管理
// ============================================================================
//
// 解析 YAML 配置文件
// ============================================================================

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

// ============================================================================
// 导入 protocol 类型
// ============================================================================

// Re-export GameRules and related types from protocol
pub use cyber_jianghu_protocol::{AvailableAction, GameRules, InitialItem};

// ============================================================================
// 配置结构
// ============================================================================

/// 人设配置（用于验证器）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaConfig {
    /// 性别
    #[serde(default = "default_gender")]
    pub gender: String,

    /// 初始年龄
    #[serde(default = "default_initial_age")]
    pub initial_age: u8,

    /// 性格特点
    #[serde(default = "default_personality")]
    pub personality: Vec<String>,

    /// 三观倾向
    #[serde(default = "default_values")]
    pub values: Vec<String>,
}

impl Default for PersonaConfig {
    fn default() -> Self {
        Self {
            gender: default_gender(),
            initial_age: default_initial_age(),
            personality: default_personality(),
            values: default_values(),
        }
    }
}

fn default_gender() -> String {
    "男".to_string()
}
fn default_initial_age() -> u8 {
    28
}
fn default_personality() -> Vec<String> {
    vec!["沉稳".into(), "重情义".into()]
}
fn default_values() -> Vec<String> {
    vec!["江湖道义为先".into()]
}

// ============================================================================
// 角色和审查配置
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
///
/// 定义 Player Agent 如何被 Observer Agent 审查
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

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            timeout_seconds: default_review_timeout(),
            enabled: default_review_enabled(),
            auth_token: None,
        }
    }
}

fn default_review_timeout() -> u64 {
    30
}
fn default_review_enabled() -> bool {
    true
}

/// 观察者配置（仅 observer 角色使用）
///
/// 定义 Observer Agent 如何连接到 Player Agent 进行审查
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

impl Default for ObserverConfig {
    fn default() -> Self {
        Self {
            target_endpoint: "http://localhost:23340".to_string(),
            auth_token: None,
            poll_interval_seconds: default_poll_interval(),
        }
    }
}

fn default_poll_interval() -> u64 {
    5
}

// ============================================================================
// Agent 配置
// ============================================================================

/// Agent 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Agent 名称
    pub name: String,

    /// 系统提示词（人设）
    pub system_prompt: String,

    /// 人设配置（用于验证器）
    #[serde(default)]
    pub persona: PersonaConfig,

    /// 记忆系统配置
    #[serde(default)]
    pub memory: MemoryConfig,

    /// Agent 角色
    #[serde(default)]
    pub role: AgentRole,

    /// 审查配置（仅 player 角色使用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewConfig>,

    /// 观察者配置（仅 observer 角色使用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observer: Option<ObserverConfig>,
}

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

    /// 数据库存储路径（默认 ~/.cyber-jianghu/）
    #[serde(default)]
    pub db_path: Option<String>,
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

fn default_memory_enabled() -> bool {
    true
}
fn default_working_memory_size() -> usize {
    20
}
fn default_episodic_threshold() -> f32 {
    0.5
}

/// 服务端配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// WebSocket URL
    pub ws_url: String,

    /// 认证 token
    pub auth_token: String,
}

impl ServerConfig {
    /// 生成带 token 的完整 WebSocket URL
    pub fn ws_url_with_token(&self) -> String {
        format!("{}?token={}", self.ws_url, self.auth_token)
    }
}

/// 完整配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Agent 配置
    pub agent: AgentConfig,

    /// 服务端配置
    pub server: ServerConfig,

    /// 记忆系统配置
    #[serde(default)]
    pub memory: MemoryConfig,

    /// 游戏规则（从服务端获取）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_rules: Option<GameRules>,
}

impl Config {
    /// 从文件加载配置
    ///
    /// 支持环境变量替换，格式为 ${VAR_NAME}
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_display = path.as_ref().display().to_string();
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {:?}", path_display))?;

        let mut config: Config =
            serde_yaml::from_str(&content).with_context(|| "Failed to parse config file")?;

        // 替换环境变量
        config.server.auth_token = Self::expand_env(&config.server.auth_token);
        config.server.ws_url = Self::expand_env(&config.server.ws_url);

        Ok(config)
    }

    /// 展开环境变量
    ///
    /// 将 ${VAR_NAME} 格式的字符串替换为实际的环境变量值
    /// 如果找不到环境变量，则保持原样（如果是部分替换则较复杂，这里简化处理）
    /// 如果字符串完全匹配 ${VAR}，则替换为值，若未设置则为空
    fn expand_env(s: &str) -> String {
        if s.starts_with("${") && s.ends_with("}") {
            let var_name = &s[2..s.len() - 1];
            // 如果环境变量存在，则替换；否则返回原字符串（或者是空字符串？原逻辑是 unwrap_or_default() 即空串）
            // 之前的逻辑是 unwrap_or_default()，这意味着未设置会导致变成空字符串
            // 对于 auth_token 没问题，对于 ws_url 可能会有问题如果没设置
            // 但既然用了 ${VAR}，就应该期望被替换。
            // 让我们保持原逻辑，如果没设置就是空串，这样会报错，提醒用户设置。
            // 但为了更好体验，如果是 ws_url，可以给个默认值？
            // 不，config.rs 不应该负责默认值策略，只负责解析。

            // 稍作修改：支持默认值 ${VAR:-default} ?
            // 目前只支持简单的 ${VAR}

            std::env::var(var_name).unwrap_or_else(|_| {
                // 如果是 ws_url，未设置可能不应该为空，而是保留原样？
                // 不，如果是 ${VAR} 格式，说明用户意图就是用变量。
                // 如果变量不存在，返回空字符串是合理的行为（表示缺失）。
                "".to_string()
            })
        } else {
            s.to_string()
        }
    }

    /// 从环境变量加载配置
    ///
    /// 使用 CYBER_JIANGHU_ 前缀避免与其他系统冲突
    pub fn from_env() -> Result<Self> {
        // 解析角色
        let role = std::env::var("CYBER_JIANGHU_AGENT_ROLE")
            .ok()
            .and_then(|r| match r.to_lowercase().as_str() {
                "player" => Some(AgentRole::Player),
                "observer" => Some(AgentRole::Observer),
                _ => None,
            })
            .unwrap_or_default();

        // 解析观察者配置（仅 observer 角色）
        let observer_config = if role == AgentRole::Observer {
            Some(ObserverConfig {
                target_endpoint: std::env::var("CYBER_JIANGHU_OBSERVER_TARGET_ENDPOINT")
                    .unwrap_or_else(|_| "http://localhost:23340".to_string()),
                auth_token: std::env::var("CYBER_JIANGHU_OBSERVER_AUTH_TOKEN").ok(),
                poll_interval_seconds: std::env::var("CYBER_JIANGHU_OBSERVER_POLL_INTERVAL")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(5),
            })
        } else {
            None
        };

        // 解析审查配置（仅 player 角色）
        let review_config = if role == AgentRole::Player {
            Some(ReviewConfig {
                timeout_seconds: std::env::var("CYBER_JIANGHU_REVIEW_TIMEOUT")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(30),
                enabled: std::env::var("CYBER_JIANGHU_REVIEW_ENABLED")
                    .ok()
                    .map(|s| s.to_lowercase() != "false")
                    .unwrap_or(true),
                auth_token: std::env::var("CYBER_JIANGHU_REVIEW_AUTH_TOKEN").ok(),
            })
        } else {
            None
        };

        Ok(Config {
            agent: AgentConfig {
                name: std::env::var("CYBER_JIANGHU_AGENT_NAME")
                    .unwrap_or_else(|_| "未命名Agent".to_string()),
                system_prompt: std::env::var("CYBER_JIANGHU_SYSTEM_PROMPT")
                    .unwrap_or_else(|_| "你是一个普通的江湖人物".to_string()),
                persona: PersonaConfig::default(),
                memory: MemoryConfig::default(),
                role,
                review: review_config,
                observer: observer_config,
            },
            server: ServerConfig {
                ws_url: std::env::var("CYBER_JIANGHU_SERVER_URL")
                    .unwrap_or_else(|_| "ws://localhost:23333/ws".to_string()),
                auth_token: std::env::var("CYBER_JIANGHU_AUTH_TOKEN")
                    .context("CYBER_JIANGHU_AUTH_TOKEN is required")?,
            },
            memory: MemoryConfig::default(),
            game_rules: None, // 从环境变量加载时不包含游戏规则，需从服务端获取
        })
    }

    /// 生成 WebSocket URL（带 token）
    pub fn ws_url_with_token(&self) -> String {
        format!("{}?token={}", self.server.ws_url, self.server.auth_token)
    }

    /// 更新游戏规则
    pub fn update_game_rules(&mut self, game_rules: GameRules) {
        self.game_rules = Some(game_rules);
    }

    /// 保存配置到文件
    ///
    /// 将配置（包括游戏规则）保存到指定的 YAML 文件
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path_display = path.as_ref().display().to_string();
        let yaml =
            serde_yaml::to_string(self).with_context(|| "Failed to serialize config to YAML")?;

        fs::write(&path, yaml)
            .with_context(|| format!("Failed to write config file: {:?}", path_display))?;

        Ok(())
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_expand_env() {
        unsafe {
            std::env::set_var("TEST_VAR", "test_value");
        }

        let result = Config::expand_env("${TEST_VAR}");
        assert_eq!(result, "test_value");

        let result = Config::expand_env("plain_string");
        assert_eq!(result, "plain_string");
    }
}
