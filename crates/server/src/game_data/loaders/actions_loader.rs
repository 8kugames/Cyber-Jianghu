// ============================================================================
// OpenClaw Cyber-Jianghu 行动配置加载器
// ============================================================================
//
// 本模块负责加载行动配置（actions.yaml 或 actions.json）
// ============================================================================

use crate::game_data::loaders::config_format::load_config;
use crate::game_data::types::UnifiedActionsConfig;
use anyhow::{Context, Result};
use std::path::Path;

/// 加载行动配置
///
/// 优先加载 YAML 格式，回退到 JSON 格式。
///
/// # 参数
/// - `config_dir`: 配置文件目录路径
///
/// # 返回
/// 统一行动配置对象
pub fn load_actions<P: AsRef<Path>>(config_dir: P) -> Result<UnifiedActionsConfig> {
    let config_dir = config_dir.as_ref();

    // 优先尝试 YAML 格式
    let yaml_path = config_dir.join("actions.yaml");
    if yaml_path.exists() {
        return load_config(&yaml_path).context("加载行动配置 (YAML) 失败");
    }

    // 回退到 JSON 格式
    let json_path = config_dir.join("actions.json");
    load_config(&json_path).context("加载行动配置 (JSON) 失败")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_data::loaders::config_format::{ConfigFormat, parse_config};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_actions_json() {
        let dir = TempDir::new().unwrap();

        // 创建测试配置文件
        fs::write(
            dir.path().join("actions.json"),
            r#"{
                "version": "2.0.0",
                "description": "行动配置文件",
                "meta": {},
                "data": {
                    "攻击": {
                        "description": "攻击目标",
                        "base_damage": 10,
                        "requirements": []
                    },
                    "偷窃": {
                        "description": "偷取物品",
                        "success_rate": 0.5,
                        "requirements": []
                    }
                }
            }"#,
        )
        .unwrap();

        let actions = load_actions(dir.path()).unwrap();
        assert_eq!(actions.version, "2.0.0");
        assert!(actions.data.contains_key("攻击"));
        assert!(actions.data.contains_key("偷窃"));
    }

    #[test]
    fn test_load_actions_yaml() {
        let yaml = r#"
version: "2.0.0"
description: "行动配置文件"
meta: {}
data:
  攻击:
    description: "攻击目标"
    base_damage: 10
    requirements: []
  偷窃:
    description: "偷取物品"
    success_rate: 0.5
    requirements: []
"#;

        let config: UnifiedActionsConfig = parse_config(yaml, ConfigFormat::Yaml).unwrap();
        assert_eq!(config.version, "2.0.0");
        assert!(config.data.contains_key("攻击"));
        assert!(config.data.contains_key("偷窃"));
    }
}
