// ============================================================================
// 生存 Reward 配置加载器
// ============================================================================
//
// 强制配置：reward.yaml 缺失即 Err 中止启动，杜绝硬编码 fallback。
// 对齐 display_messages_loader.rs 的 fail-fast 先例。
// ============================================================================

use crate::game_data::loaders::config_format::load_config;
use crate::game_data::types::reward::RewardConfig;
use anyhow::{Context, Result};
use std::path::Path;

/// 加载 reward 配置（强制，缺失即 Err 中止启动）
///
/// 优先加载 YAML 格式，回退 JSON 格式。两者均缺失则 fail-fast。
pub fn load_reward<P: AsRef<Path>>(config_dir: P) -> Result<RewardConfig> {
    let config_dir = config_dir.as_ref();

    // 优先 YAML
    let yaml_path = config_dir.join("reward.yaml");
    if yaml_path.exists() {
        return load_config(&yaml_path).context("加载 reward 配置 (YAML) 失败");
    }

    // 回退 JSON
    let json_path = config_dir.join("reward.json");
    if json_path.exists() {
        return load_config(&json_path).context("加载 reward 配置 (JSON) 失败");
    }

    // fail-fast：两者均缺失，对齐 display_messages_loader.rs:28 先例
    Err(anyhow::anyhow!(
        "[reward_loader] reward 配置文件不存在: {:?} 或 {:?}（reward 为强制配置，杜绝硬编码 fallback）",
        yaml_path,
        json_path
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_reward_yaml() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("reward.yaml"),
            r#"
version: "0.0.1"
daily:
  survival_score: 1.0
  physiological:
    satiation_weight: 0.25
    hydration_weight: 0.25
lifetime:
  death_penalty: -50.0
"#,
        )
        .unwrap();
        let cfg = load_reward(dir.path()).unwrap();
        assert_eq!(cfg.daily.survival_score, 1.0);
        assert_eq!(cfg.lifetime.death_penalty, -50.0);
    }

    #[test]
    fn test_load_reward_missing_fail_fast() {
        // P1-1 验收：配置缺失必须 Err（非静默 None）
        let dir = TempDir::new().unwrap();
        let result = load_reward(dir.path());
        assert!(result.is_err(), "缺失 reward 配置必须 fail-fast 返回 Err");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("reward") && err_msg.contains("强制"),
            "错误信息应说明 reward 为强制配置，got: {}",
            err_msg
        );
    }
}
