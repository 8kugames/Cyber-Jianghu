use anyhow::{Context, Result};
use std::path::Path;

use crate::game_data::types::UnifiedTimeConfig;

/// 加载时间配置
///
/// 从指定的 JSON 文件加载时间与季节配置数据。
///
/// # 参数
/// * `path` - 配置文件路径
///
/// # 返回
/// * `Result<UnifiedTimeConfig>` - 加载成功返回配置对象，失败返回错误
pub fn load_time<P: AsRef<Path>>(path: P) -> Result<UnifiedTimeConfig> {
    let path = path.as_ref();
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取时间配置文件: {}", path.display()))?;

    let config: UnifiedTimeConfig = serde_json::from_str(&content)
        .with_context(|| format!("解析时间配置文件失败: {}", path.display()))?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_time() {
        let json = r#"{
            "version": "2.0.0",
            "description": "Test",
            "meta": {},
            "data": {
                "ticks_per_hour": 60,
                "hours_per_day": 24,
                "days_per_season": 10,
                "seasons": []
            }
        }"#;

        let config: UnifiedTimeConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.version, "2.0.0");
        assert_eq!(config.data.ticks_per_hour, 60);
    }
}
