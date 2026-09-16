use anyhow::{Context, Result};
use std::path::Path;

use crate::game_data::loaders::config_format::load_config;
use crate::game_data::types::UnifiedTimeConfig;

/// 加载时间配置
///
/// 从指定的配置文件加载时间与季节配置数据。
/// 支持 JSON (.json) 和 YAML (.yaml/.yml) 格式。
///
/// # 参数
/// * `path` - 配置文件路径
///
/// # 返回
/// * `Result<UnifiedTimeConfig>` - 加载成功返回配置对象，失败返回错误
pub fn load_time<P: AsRef<Path>>(path: P) -> Result<UnifiedTimeConfig> {
    let config: UnifiedTimeConfig = load_config(path).context("加载时间配置失败")?;
    config
        .data
        .validate()
        .map_err(|e| anyhow::anyhow!("时间配置季节乘数校验失败（拒绝加载非法数值）: {e}"))?;
    Ok(config)
}

/// 从目录加载时间配置（优先 YAML，回退 JSON）（预留：多目录配置加载）
///
/// # 参数
/// * `config_dir` - 配置目录路径
///
/// # 返回
/// * `Result<UnifiedTimeConfig>` - 加载成功返回配置对象
#[allow(dead_code)]
pub fn load_time_from_dir<P: AsRef<Path>>(config_dir: P) -> Result<UnifiedTimeConfig> {
    let config_dir = config_dir.as_ref();

    // 优先尝试 YAML 格式
    let yaml_path = config_dir.join("time.yaml");
    if yaml_path.exists() {
        let config: UnifiedTimeConfig =
            load_config(&yaml_path).context("加载时间配置 (YAML) 失败")?;
        config
            .data
            .validate()
            .map_err(|e| anyhow::anyhow!("时间配置季节乘数校验失败（拒绝加载非法数值）: {e}"))?;
        return Ok(config);
    }

    // 回退到 JSON 格式
    let json_path = config_dir.join("time.json");
    let config: UnifiedTimeConfig = load_config(&json_path).context("加载时间配置 (JSON) 失败")?;
    config
        .data
        .validate()
        .map_err(|e| anyhow::anyhow!("时间配置季节乘数校验失败（拒绝加载非法数值）: {e}"))?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_data::loaders::config_format::{ConfigFormat, parse_config};

    #[test]
    fn test_load_time_json() {
        let json = r#"{
            "version": "2.0.0",
            "description": "Test",
            "meta": {},
            "data": {
                "ticks_per_hour": 1,
                "hours_per_day": 24,
                "days_per_season": 10,
                "seasons_per_year": 4,
                "seasons": []
            }
        }"#;

        let config: UnifiedTimeConfig = parse_config(json, ConfigFormat::Json).unwrap();
        assert_eq!(config.version, "2.0.0");
        assert_eq!(config.data.ticks_per_hour, 1);
    }

    #[test]
    fn test_validate_rejects_negative_growth_rate() {
        let json = r#"{
            "version": "2.0.0",
            "description": "Test",
            "meta": {},
            "data": {
                "ticks_per_hour": 1,
                "hours_per_day": 24,
                "days_per_season": 10,
                "seasons_per_year": 4,
                "seasons": [
                    {
                        "id": "winter",
                        "name": "冬",
                        "description": "",
                        "temperature_modifier": 0,
                        "resource_growth_rate": -0.5,
                        "attribute_modifiers": {}
                    }
                ]
            }
        }"#;
        let config: UnifiedTimeConfig = parse_config(json, ConfigFormat::Json).unwrap();
        let err = config.data.validate().unwrap_err();
        assert!(err.contains("resource_growth_rate"), "err: {err}");
    }

    #[test]
    fn test_validate_rejects_negative_attribute_modifier() {
        let json = r#"{
            "version": "2.0.0",
            "description": "Test",
            "meta": {},
            "data": {
                "ticks_per_hour": 1,
                "hours_per_day": 24,
                "days_per_season": 10,
                "seasons_per_year": 4,
                "seasons": [
                    {
                        "id": "winter",
                        "name": "冬",
                        "description": "",
                        "temperature_modifier": 0,
                        "resource_growth_rate": 0.2,
                        "attribute_modifiers": { "satiation": -1.0 }
                    }
                ]
            }
        }"#;
        let config: UnifiedTimeConfig = parse_config(json, ConfigFormat::Json).unwrap();
        let err = config.data.validate().unwrap_err();
        assert!(err.contains("attribute_modifiers"), "err: {err}");
    }

    #[test]
    fn test_validate_accepts_production_shape() {
        let json = r#"{
            "version": "2.0.0",
            "description": "Test",
            "meta": {},
            "data": {
                "ticks_per_hour": 1,
                "hours_per_day": 24,
                "days_per_season": 10,
                "seasons_per_year": 4,
                "seasons": [
                    {
                        "id": "winter",
                        "name": "冬",
                        "description": "",
                        "temperature_modifier": -10,
                        "resource_growth_rate": 0.2,
                        "attribute_modifiers": { "satiation": 1.5, "hydration": 1.5 }
                    }
                ]
            }
        }"#;
        let config: UnifiedTimeConfig = parse_config(json, ConfigFormat::Json).unwrap();
        assert!(config.data.validate().is_ok());
    }

    #[test]
    fn test_load_time_yaml() {
        let yaml = r#"
version: "2.0.0"
description: "Test"
meta: {}
data:
  ticks_per_hour: 1
  hours_per_day: 24
  days_per_season: 10
  seasons_per_year: 4
  seasons: []
"#;

        let config: UnifiedTimeConfig = parse_config(yaml, ConfigFormat::Yaml).unwrap();
        assert_eq!(config.version, "2.0.0");
        assert_eq!(config.data.ticks_per_hour, 1);
    }
}
