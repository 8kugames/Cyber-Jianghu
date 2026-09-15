// ============================================================================
// OpenClaw 配置文件（~/.openclaw/openclaw.json）读取
// ============================================================================

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

/// OpenClaw 配置文件格式
#[derive(Debug, Deserialize)]
pub struct OpenClawConfig {
    /// Gateway 配置
    #[serde(default)]
    gateway: Option<GatewayConfig>,
}

#[derive(Debug, Deserialize)]
struct GatewayConfig {
    /// Gateway 地址
    url: Option<String>,
}

impl OpenClawConfig {
    /// 从默认路径读取配置
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;
        let content = std::fs::read_to_string(&config_path).with_context(|| {
            format!(
                "Failed to read OpenClaw config from {}",
                config_path.display()
            )
        })?;
        serde_json::from_str(&content).with_context(|| {
            format!(
                "Failed to parse OpenClaw config from {}",
                config_path.display()
            )
        })
    }

    /// 获取配置文件路径
    pub fn config_path() -> Result<PathBuf> {
        let config_dir = std::env::var("HOME")
            .map(|home| PathBuf::from(home).join(".openclaw"))
            .unwrap_or_else(|_| PathBuf::from("."));

        Ok(config_dir.join("openclaw.json"))
    }

    /// 获取 Gateway URL
    pub fn gateway_url(&self) -> Option<&String> {
        self.gateway.as_ref()?.url.as_ref()
    }
}
