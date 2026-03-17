use anyhow::{Context, Result};
use std::path::Path;

use crate::game_data::types::UnifiedRecipesConfig;

/// 加载配方配置
///
/// 从指定的 JSON 文件加载配方配置数据。
///
/// # 参数
/// * `path` - 配置文件路径
///
/// # 返回
/// * `Result<UnifiedRecipesConfig>` - 加载成功返回配置对象，失败返回错误
pub fn load_recipes<P: AsRef<Path>>(path: P) -> Result<UnifiedRecipesConfig> {
    let path = path.as_ref();
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取配方配置文件: {}", path.display()))?;

    let config: UnifiedRecipesConfig = serde_json::from_str(&content)
        .with_context(|| format!("解析配方配置文件失败: {}", path.display()))?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_recipes() {
        let json = r#"{
            "version": "2.0.0",
            "description": "配方配置文件",
            "meta": {},
            "data": {
                "test_recipe": {
                    "name": "测试配方",
                    "description": "测试描述",
                    "result_item": "test_item",
                    "result_quantity": 1,
                    "materials": [],
                    "tools": [],
                    "stamina_cost": 5
                }
            }
        }"#;

        let config: UnifiedRecipesConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.version, "2.0.0");
        assert!(config.data.contains_key("test_recipe"));
    }
}
