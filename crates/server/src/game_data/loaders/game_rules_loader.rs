// ============================================================================
// OpenClaw Cyber-Jianghu 游戏规则配置加载器
// ============================================================================
//
// 本模块负责加载游戏规则配置（game_rules.yaml 或 game_rules.json）
// ============================================================================

use crate::game_data::loaders::config_format::load_config;
use crate::game_data::types::UnifiedGameRulesConfig;
use anyhow::{Context, Result};
use std::path::Path;

/// 加载游戏规则配置
///
/// 优先加载 YAML 格式，回退到 JSON 格式。
///
/// # 参数
/// - `config_dir`: 配置文件目录路径
///
/// # 返回
/// 统一游戏规则配置对象
pub fn load_game_rules<P: AsRef<Path>>(config_dir: P) -> Result<UnifiedGameRulesConfig> {
    let config_dir = config_dir.as_ref();

    // 优先尝试 YAML 格式
    let yaml_path = config_dir.join("game_rules.yaml");
    if yaml_path.exists() {
        return load_config(&yaml_path).context("加载游戏规则配置 (YAML) 失败");
    }

    // 回退到 JSON 格式
    let json_path = config_dir.join("game_rules.json");
    load_config(&json_path).context("加载游戏规则配置 (JSON) 失败")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_data::loaders::config_format::{parse_config, ConfigFormat};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_game_rules_json() {
        let dir = TempDir::new().unwrap();

        // 创建测试配置文件
        fs::write(
            dir.path().join("game_rules.json"),
            r#"{
                "version": "2.0.0",
                "description": "游戏规则配置文件",
                "meta": {},
                "data": {
                    "agent_state": {
                        "tick": {
                            "real_seconds_per_tick": 60
                        },
                        "location": {
                            "spawn_location": "longmen_inn"
                        },
                        "game_time": {
                            "start_date": "2024-01-01"
                        }
                    },
                    "validation": {
                        "action_validation": {
                            "max_content_length": 1000
                        },
                        "max_agent_name_length": 100,
                        "max_system_prompt_length": 102400,
                        "max_speak_content_length": 500
                    },
                    "ops": {
                        "death_threshold": 10,
                        "offline_cleanup_days": 30
                    }
                }
            }"#,
        )
        .unwrap();

        let rules = load_game_rules(dir.path()).unwrap();
        assert_eq!(rules.version, "2.0.0");
        assert_eq!(rules.data.agent_state.tick.real_seconds_per_tick, 60);
    }

    #[test]
    fn test_load_game_rules_yaml() {
        let yaml = r#"
version: "2.0.0"
description: "游戏规则配置文件"
meta: {}
data:
  agent_state:
    tick:
      real_seconds_per_tick: 60
    location:
      spawn_location: "longmen_inn"
    game_time:
      start_date: "2024-01-01"
  validation:
    action_validation:
      max_content_length: 1000
    max_agent_name_length: 100
    max_system_prompt_length: 102400
    max_speak_content_length: 500
  ops:
    death_threshold: 10
    offline_cleanup_days: 30
"#;

        let config: UnifiedGameRulesConfig = parse_config(yaml, ConfigFormat::Yaml).unwrap();
        assert_eq!(config.version, "2.0.0");
        assert_eq!(config.data.agent_state.tick.real_seconds_per_tick, 60);
    }
}
