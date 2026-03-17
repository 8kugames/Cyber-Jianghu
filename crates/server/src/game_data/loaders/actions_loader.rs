// ============================================================================
// OpenClaw Cyber-Jianghu 行动配置加载器
// ============================================================================
//
// 本模块负责加载行动配置（actions.json）
// ============================================================================

use crate::game_data::types::UnifiedActionsConfig;
use anyhow::{Context, Result};
use std::path::Path;

/// 加载行动配置
///
/// # 参数
/// - `config_dir`: 配置文件目录路径
///
/// # 返回
/// 统一行动配置对象
pub fn load_actions<P: AsRef<Path>>(config_dir: P) -> Result<UnifiedActionsConfig> {
    let file_path = config_dir.as_ref().join("actions.json");

    let content = std::fs::read_to_string(&file_path)
        .with_context(|| format!("无法读取配置文件: {}", file_path.display()))?;

    let config: UnifiedActionsConfig = serde_json::from_str(&content)
        .with_context(|| format!("解析配置文件失败: {}", file_path.display()))?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_actions() {
        let dir = TempDir::new().unwrap();

        // 创建测试配置文件
        fs::write(
            dir.path().join("actions.json"),
            r#"{
                "version": "2.0.0",
                "description": "行动配置文件",
                "meta": {},
                "data": {
                    "attack": {
                        "description": "攻击目标",
                        "base_damage": 10,
                        "requirements": []
                    },
                    "steal": {
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
        assert!(actions.data.contains_key("attack"));
        assert!(actions.data.contains_key("steal"));
    }
}
