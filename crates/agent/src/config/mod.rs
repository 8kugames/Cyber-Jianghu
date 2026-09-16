// ============================================================================
// 配置管理
// ============================================================================
//
// Agent 配置结构，分为三层：
// 1. Identity - Agent 身份（持久化，不随角色变化）
// 2. Server - 服务器连接配置
// 3. Character - 当前角色（通过 Web/API 创建）
// ============================================================================

use crate::component::memory::types::EbbinghausConfig;
use crate::component::persona::PersonaPersistenceConfig;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use url::Url;
use uuid::Uuid;
use zeroize::Zeroize;

// ============================================================================
// 导入 protocol 类型
// ============================================================================

pub use cyber_jianghu_protocol::{AvailableAction, GameRules, InitialItem, WorldTime};

// ============================================================================
// 路径解析函数
// ============================================================================

/// 返回配置目录路径（优先 CYBER_JIANGHU_CONFIG_DIR 环境变量，回退 $HOME/.cyber-jianghu/config）
pub fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CYBER_JIANGHU_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cyber-jianghu")
        .join("config")
}

/// 返回数据目录路径（优先 CYBER_JIANGHU_DATA_DIR 环境变量，回退 $HOME/.cyber-jianghu）
pub fn data_base_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CYBER_JIANGHU_DATA_DIR") {
        return PathBuf::from(dir);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cyber-jianghu")
}

/// 加载 `persona.yaml`（缺失/字段缺失一律回退到默认 (10, true, true)）。
pub fn load_persona_persistence_config(config_dir: &Path) -> PersonaPersistenceConfig {
    let path = config_dir.join("persona.yaml");
    if !path.exists() {
        return PersonaPersistenceConfig::default();
    }
    match fs::read_to_string(&path) {
        Ok(content) => match serde_yaml::from_str::<PersonaPersistenceConfig>(&content) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!(
                    "[config] persona.yaml 解析失败: {}，回退到默认 (10, true, true)",
                    e
                );
                PersonaPersistenceConfig::default()
            }
        },
        Err(e) => {
            eprintln!("[config] 读取 persona.yaml 失败: {}，回退到默认", e);
            PersonaPersistenceConfig::default()
        }
    }
}

/// 保存 narrative_config 到磁盘（hash skip-optimization）
///
/// 比较新旧 hash，相同则跳过写入。写入成功后同步保存 .hash 文件。
pub fn save_narrative_config_to_disk(
    config: &cyber_jianghu_protocol::NarrativeConfig,
    hash: Option<&str>,
) -> std::io::Result<()> {
    let cdir = config_dir();
    let hash_path = cdir.join("narrative_config.hash");

    let should_save = match hash {
        Some(new_hash) => match std::fs::read_to_string(&hash_path) {
            Ok(old_hash) => old_hash.trim() != new_hash,
            Err(_) => true,
        },
        None => true,
    };

    if !should_save {
        return Ok(());
    }

    std::fs::create_dir_all(&cdir)?;
    let json = serde_json::to_string_pretty(config)?;
    std::fs::write(cdir.join("narrative_config.json"), json)?;
    if let Some(h) = hash {
        std::fs::write(&hash_path, h)?;
    }
    Ok(())
}

// ============================================================================
// 每服务器设备身份配置（device.yaml）
// ============================================================================

/// 每服务器设备身份（device.yaml）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub device_id: Uuid,
    pub auth_token: String,
    pub server_url: String,
}

impl DeviceConfig {
    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        let yaml = serde_yaml::to_string(self).context("Failed to serialize DeviceConfig")?;
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, &yaml)?;
        fs::rename(&tmp_path, path)?;
        Ok(())
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let yaml = fs::read_to_string(path).context("Failed to read device.yaml")?;
        serde_yaml::from_str(&yaml).context("Failed to parse device.yaml")
    }

    pub fn ws_url_with_token(&self, ws_url: &str, agent_id: Option<Uuid>) -> String {
        let mut url = format!(
            "{}?device_id={}&token={}",
            ws_url, self.device_id, self.auth_token
        );
        if let Some(id) = agent_id {
            url.push_str(&format!("&agent_id={}", id));
        }
        url
    }
}

/// 计算服务器目录 key（从 WebSocket URL 派生）
pub fn server_key(ws_url: &str) -> String {
    let url = Url::parse(ws_url).unwrap_or_else(|_| {
        Url::parse(&format!("ws://{}", ws_url)).expect("ws:// prefix always produces valid URL")
    });
    let host = url.host_str().unwrap_or("localhost");
    let port = url.port().map(|p| format!("-{}", p)).unwrap_or_default();
    format!("{}{}", host.replace(['.', ':', '[', ']'], "-"), port)
}

/// Convert WebSocket URL to HTTP URL.
/// e.g. `ws://localhost:23333/ws` -> `http://localhost:23333`
pub fn ws_to_http_url(ws_url: &str) -> String {
    ws_url
        .replace("ws://", "http://")
        .replace("wss://", "https://")
        .rsplit_once('/')
        .map(|(base, _)| base.to_string())
        .unwrap_or_else(|| ws_url.to_string())
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
    pub fn ws_url_with_token(
        &self,
        device_id: Uuid,
        auth_token: &str,
        agent_id: Option<Uuid>,
    ) -> String {
        let mut url = format!(
            "{}?device_id={}&token={}",
            self.ws_url, device_id, auth_token
        );
        if let Some(id) = agent_id {
            url.push_str(&format!("&agent_id={}", id));
        }
        url
    }
}

// ============================================================================
// 运行时配置
// ============================================================================

/// 运行模式
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeMode {
    /// Cognitive 模式（默认）- 内置 LLM 决策，无需外部调度器
    #[default]
    Cognitive,
    /// Claw 模式 - 为 OpenClaw 等外部助手提供 WebSocket + HTTP API
    /// LLM 由外部 OpenClaw 提供，Agent 内部认知引擎通过 OpenClawBridge 调用
    Claw,
}

impl std::fmt::Display for RuntimeMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeMode::Claw => write!(f, "claw"),
            RuntimeMode::Cognitive => write!(f, "cognitive"),
        }
    }
}

fn default_token_opt_enabled() -> bool {
    true
}

fn default_true() -> bool {
    true
}

/// 运行时配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    /// 运行模式
    #[serde(default)]
    pub mode: RuntimeMode,

    /// HTTP API 端口
    /// 0 = 在 23340~23999 范围内随机选择
    #[serde(default)]
    pub port: u16,

    /// 停止 LLM 调用
    #[serde(default)]
    pub llm_disabled: bool,

    /// 自动重生开关：角色死亡后自动转世重生（复用角色信息）
    #[serde(default = "default_true")]
    pub auto_rebirth: bool,

    /// 等待注册超时后自动生成角色（秒）。0 = 禁用；默认 1800（30 分钟）。
    /// 触发条件：启动时无可自动转世角色（全新安装/全部归隐）。
    /// 等待期面板显示倒计时引导人工注册，超时由 Agent 自行生成并注册。
    #[serde(default = "default_auto_register_timeout_secs")]
    pub auto_register_timeout_secs: u64,
}

fn default_auto_register_timeout_secs() -> u64 {
    1800
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            mode: RuntimeMode::Cognitive,
            port: 0,
            llm_disabled: false,
            auto_rebirth: true,
            auto_register_timeout_secs: default_auto_register_timeout_secs(),
        }
    }
}

mod character;
mod llm;
mod sub_configs;

pub use character::*;
pub use llm::*;
pub use sub_configs::*;

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
