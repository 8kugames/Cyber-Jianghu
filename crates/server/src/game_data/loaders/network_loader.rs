// ============================================================================
// OpenClaw Cyber-Jianghu 网络配置加载器
// ============================================================================
//
// 本模块负责加载网络配置（network.yaml 或 network.json）
// ============================================================================

use crate::game_data::loaders::config_format::load_config;
use crate::game_data::types::UnifiedNetworkConfig;
use anyhow::{Context, Result};
use std::path::Path;

/// 加载网络配置
///
/// 优先加载 YAML 格式，回退到 JSON 格式。
///
/// # 参数
/// - `config_dir`: 配置文件目录路径
///
/// # 返回
/// 统一网络配置对象
pub fn load_network<P: AsRef<Path>>(config_dir: P) -> Result<UnifiedNetworkConfig> {
    let config_dir = config_dir.as_ref();

    // 优先尝试 YAML 格式
    let yaml_path = config_dir.join("network.yaml");
    if yaml_path.exists() {
        return load_config(&yaml_path).context("加载网络配置 (YAML) 失败");
    }

    // 回退到 JSON 格式
    let json_path = config_dir.join("network.json");
    load_config(&json_path).context("加载网络配置 (JSON) 失败")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_data::loaders::config_format::{ConfigFormat, parse_config};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_network_json() {
        let dir = TempDir::new().unwrap();

        // 创建测试配置文件
        fs::write(
            dir.path().join("network.json"),
            r#"{
                "version": "2.0.0",
                "description": "网络配置文件",
                "meta": {},
                "data": {
                    "websocket": {
                        "rate_limit_ms": 500,
                        "cleanup_interval_secs": 300,
                        "cleanup_threshold": 100
                    },
                    "dialogue": {
                        "window_seconds": 300,
                        "max_messages_per_agent": 20
                    }
                }
            }"#,
        )
        .unwrap();

        let config = load_network(dir.path()).unwrap();
        assert_eq!(config.version, "2.0.0");
        assert_eq!(config.data.websocket.rate_limit_ms, 500);
        assert_eq!(config.data.websocket.cleanup_interval_secs, 300);
        assert_eq!(config.data.websocket.cleanup_threshold, 100);
    }

    #[test]
    fn test_load_network_yaml() {
        let yaml = r#"
version: "2.0.0"
description: "网络配置文件"
meta: {}
data:
  websocket:
    rate_limit_ms: 500
    cleanup_interval_secs: 300
    cleanup_threshold: 100
  dialogue:
    window_seconds: 300
    max_messages_per_agent: 20
"#;

        let config: UnifiedNetworkConfig = parse_config(yaml, ConfigFormat::Yaml).unwrap();
        assert_eq!(config.version, "2.0.0");
        assert_eq!(config.data.websocket.rate_limit_ms, 500);
        assert_eq!(config.data.websocket.cleanup_interval_secs, 300);
        assert_eq!(config.data.websocket.cleanup_threshold, 100);
    }
}
