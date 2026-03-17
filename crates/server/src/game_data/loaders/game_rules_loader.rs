// ============================================================================
// OpenClaw Cyber-Jianghu 游戏规则配置加载器
// ============================================================================
//
// 本模块负责加载游戏规则配置（game_rules.json）
// ============================================================================

use crate::game_data::types::UnifiedGameRulesConfig;
use anyhow::{Context, Result};
use std::path::Path;

/// 加载游戏规则配置
///
/// # 参数
/// - `config_dir`: 配置文件目录路径
///
/// # 返回
/// 统一游戏规则配置对象
pub fn load_game_rules<P: AsRef<Path>>(config_dir: P) -> Result<UnifiedGameRulesConfig> {
    let file_path = config_dir.as_ref().join("game_rules.json");

    let content = std::fs::read_to_string(&file_path)
        .with_context(|| format!("无法读取配置文件: {}", file_path.display()))?;

    let config: UnifiedGameRulesConfig = serde_json::from_str(&content)
        .with_context(|| format!("解析配置文件失败: {}", file_path.display()))?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_game_rules() {
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
                        "decay": {
                            "sanity_per_tick": 0,
                            "hunger_per_tick": -5,
                            "thirst_per_tick": -5,
                            "stamina_recovery": 5,
                            "stamina_after_action": 2,
                            "idle_bonus": 5
                        },
                        "limits": {
                            "hp_min": 0,
                            "hp_max": 100,
                            "stamina_min": 0,
                            "stamina_max": 100,
                            "hunger_min": 0,
                            "hunger_max": 100,
                            "thirst_min": 0,
                            "thirst_max": 100,
                            "sanity_min": 0,
                            "sanity_max": 100,
                            "reputation_min": -1000,
                            "reputation_max": 1000
                        },
                        "tick": {
                            "game_hours_per_tick": 1,
                            "real_seconds_per_tick": 60
                        },
                        "location": {
                            "spawn_location": "longmen_inn"
                        },
                        "game_time": {
                            "time_ratio": 24,
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
                    }
                }
            }"#,
        )
        .unwrap();

        let rules = load_game_rules(dir.path()).unwrap();
        assert_eq!(rules.version, "2.0.0");
        assert_eq!(rules.data.agent_state.decay.hunger_per_tick, -5);
    }
}
