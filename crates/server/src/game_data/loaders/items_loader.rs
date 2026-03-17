// ============================================================================
// OpenClaw Cyber-Jianghu 物品配置加载器
// ============================================================================
//
// 本模块负责加载物品配置（items.json）
// ============================================================================

use crate::game_data::types::UnifiedItemsConfig;
use anyhow::{Context, Result};
use std::path::Path;

/// 加载物品配置
///
/// # 参数
/// - `config_dir`: 配置文件目录路径
///
/// # 返回
/// 统一物品配置对象
pub fn load_items<P: AsRef<Path>>(config_dir: P) -> Result<UnifiedItemsConfig> {
    let file_path = config_dir.as_ref().join("items.json");

    let content = std::fs::read_to_string(&file_path)
        .with_context(|| format!("无法读取配置文件: {}", file_path.display()))?;

    let config: UnifiedItemsConfig = serde_json::from_str(&content)
        .with_context(|| format!("解析配置文件失败: {}", file_path.display()))?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_items() {
        let dir = TempDir::new().unwrap();

        // 创建测试配置文件
        fs::write(
            dir.path().join("items.json"),
            r#"{
                "version": "2.0.0",
                "description": "物品配置文件",
                "meta": {},
                "data": [
                    {
                        "item_id": "mantou",
                        "name": "馒头",
                        "item_type": "consumable",
                        "effects": [
                            {
                                "attribute": "hunger",
                                "operation": "add",
                                "value": 30
                            }
                        ],
                        "stack_size": 10,
                        "description": "热腾腾的馒头"
                    }
                ]
            }"#,
        )
        .unwrap();

        let items = load_items(dir.path()).unwrap();
        assert_eq!(items.version, "2.0.0");
        assert_eq!(items.data.len(), 1);
        assert_eq!(items.data[0].item_id, "mantou");
    }
}
