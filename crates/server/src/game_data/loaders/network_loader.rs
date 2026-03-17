// ============================================================================
// OpenClaw Cyber-Jianghu 网络配置加载器
// ============================================================================
//
// 本模块负责加载网络配置（network.json）
// ============================================================================

use crate::game_data::types::UnifiedNetworkConfig;
use anyhow::{Context, Result};
use std::path::Path;

/// 加载网络配置
///
/// # 参数
/// - `config_dir`: 配置文件目录路径
///
/// # 返回
/// 统一网络配置对象
pub fn load_network<P: AsRef<Path>>(config_dir: P) -> Result<UnifiedNetworkConfig> {
    let file_path = config_dir.as_ref().join("network.json");

    let content = std::fs::read_to_string(&file_path)
        .with_context(|| format!("无法读取配置文件: {}", file_path.display()))?;

    let config: UnifiedNetworkConfig = serde_json::from_str(&content)
        .with_context(|| format!("解析配置文件失败: {}", file_path.display()))?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_network() {
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
}
