// ============================================================================
// OpenClaw Cyber-Jianghu 背包配置加载器
// ============================================================================
//
// 本模块负责加载背包相关配置：
// - initial_inventory.json (初始物品清单)
// - inventory.json (背包限制)
// ============================================================================

use crate::game_data::types::{
    UnifiedInitialInventoryConfig, UnifiedInventoryConfig,
};
use anyhow::{Context, Result};
use std::path::Path;

/// 加载初始物品配置
///
/// # 参数
/// - `config_dir`: 配置文件目录路径
///
/// # 返回
/// 统一初始物品配置对象
pub fn load_initial_inventory<P: AsRef<Path>>(config_dir: P) -> Result<UnifiedInitialInventoryConfig> {
    let file_path = config_dir.as_ref().join("initial_inventory.json");

    let content = std::fs::read_to_string(&file_path)
        .with_context(|| format!("无法读取配置文件: {}", file_path.display()))?;

    let config: UnifiedInitialInventoryConfig = serde_json::from_str(&content)
        .with_context(|| format!("解析配置文件失败: {}", file_path.display()))?;

    Ok(config)
}

/// 加载背包配置
///
/// # 参数
/// - `config_dir`: 配置文件目录路径
///
/// # 返回
/// 统一背包配置对象
pub fn load_inventory<P: AsRef<Path>>(config_dir: P) -> Result<UnifiedInventoryConfig> {
    let file_path = config_dir.as_ref().join("inventory.json");

    let content = std::fs::read_to_string(&file_path)
        .with_context(|| format!("无法读取配置文件: {}", file_path.display()))?;

    let config: UnifiedInventoryConfig = serde_json::from_str(&content)
        .with_context(|| format!("解析配置文件失败: {}", file_path.display()))?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_initial_inventory() {
        let dir = TempDir::new().unwrap();

        // 创建测试配置文件
        fs::write(
            dir.path().join("initial_inventory.json"),
            r#"{
                "version": "2.0.0",
                "description": "初始物品配置",
                "meta": {},
                "data": [
                    { "item_id": "mantou", "name": "馒头", "quantity": 3, "description": "热腾腾的馒头" }
                ]
            }"#,
        ).unwrap();

        let config = load_initial_inventory(dir.path()).unwrap();
        assert_eq!(config.version, "2.0.0");
        assert_eq!(config.data.len(), 1);
        assert_eq!(config.data[0].item_id, "mantou");
        assert_eq!(config.data[0].quantity, 3);
    }

    #[test]
    fn test_load_inventory() {
        let dir = TempDir::new().unwrap();

        // 创建测试配置文件
        fs::write(
            dir.path().join("inventory.json"),
            r#"{
                "version": "2.0.0",
                "description": "背包配置",
                "meta": {},
                "data": {
                    "max_slots": 10,
                    "max_stack_size": 10
                }
            }"#,
        )
        .unwrap();

        let config = load_inventory(dir.path()).unwrap();
        assert_eq!(config.version, "2.0.0");
        assert_eq!(config.data.max_slots, 10);
        assert_eq!(config.data.max_stack_size, 10);
    }
}
