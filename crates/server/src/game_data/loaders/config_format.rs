// ============================================================================
// 通用配置格式加载器
// ============================================================================
//
// 支持 JSON 和 YAML 两种配置格式，根据文件扩展名自动选择解析器
// ============================================================================

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use std::path::Path;

/// 配置文件格式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    Json,
    Yaml,
}

impl ConfigFormat {
    /// 从文件路径推断格式
    pub fn from_path<P: AsRef<Path>>(path: P) -> Option<Self> {
        let path = path.as_ref();
        let ext = path.extension()?.to_str()?.to_lowercase();
        match ext.as_str() {
            "json" => Some(ConfigFormat::Json),
            "yaml" | "yml" => Some(ConfigFormat::Yaml),
            _ => None,
        }
    }
}

/// 从文件加载配置（自动检测格式）
///
/// 根据文件扩展名自动选择 JSON 或 YAML 解析器
///
/// # 参数
/// * `path` - 配置文件路径（.json, .yaml, .yml）
///
/// # 返回
/// * `Result<T>` - 解析后的配置对象
pub fn load_config<P: AsRef<Path>, T: DeserializeOwned>(path: P) -> Result<T> {
    let path = path.as_ref();
    let format = ConfigFormat::from_path(path)
        .with_context(|| format!("无法识别配置文件格式: {}", path.display()))?;

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取配置文件: {}", path.display()))?;

    match format {
        ConfigFormat::Json => serde_json::from_str(&content)
            .with_context(|| format!("解析 JSON 配置失败: {}", path.display())),
        ConfigFormat::Yaml => serde_yaml::from_str(&content)
            .with_context(|| format!("解析 YAML 配置失败: {}", path.display())),
    }
}

/// 从字符串解析配置（指定格式）
///
/// # 参数
/// * `content` - 配置内容字符串
/// * `format` - 配置格式
///
/// # 返回
/// * `Result<T>` - 解析后的配置对象
pub fn parse_config<T: DeserializeOwned>(content: &str, format: ConfigFormat) -> Result<T> {
    match format {
        ConfigFormat::Json => serde_json::from_str(content).context("解析 JSON 失败"),
        ConfigFormat::Yaml => serde_yaml::from_str(content).context("解析 YAML 失败"),
    }
}

/// 序列化配置为字符串（预留：配置编辑器保存功能）
///
/// # 参数
/// * `value` - 要序列化的值
/// * `format` - 目标格式
///
/// # 返回
/// * `Result<String>` - 序列化后的字符串
#[allow(dead_code)]
pub fn serialize_config<T: serde::Serialize>(value: &T, format: ConfigFormat) -> Result<String> {
    match format {
        ConfigFormat::Json => serde_json::to_string_pretty(value).context("序列化为 JSON 失败"),
        ConfigFormat::Yaml => serde_yaml::to_string(value).context("序列化为 YAML 失败"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct TestConfig {
        version: String,
        data: TestData,
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct TestData {
        value: i32,
        name: String,
    }

    #[test]
    fn test_format_detection() {
        assert_eq!(
            ConfigFormat::from_path("config.json"),
            Some(ConfigFormat::Json)
        );
        assert_eq!(
            ConfigFormat::from_path("config.yaml"),
            Some(ConfigFormat::Yaml)
        );
        assert_eq!(
            ConfigFormat::from_path("config.yml"),
            Some(ConfigFormat::Yaml)
        );
        assert_eq!(ConfigFormat::from_path("config.txt"), None);
    }

    #[test]
    fn test_parse_json() {
        let json = r#"{"version": "1.0", "data": {"value": 42, "name": "test"}}"#;
        let config: TestConfig = parse_config(json, ConfigFormat::Json).unwrap();
        assert_eq!(config.version, "1.0");
        assert_eq!(config.data.value, 42);
    }

    #[test]
    fn test_parse_yaml() {
        let yaml = r#"
version: "1.0"
data:
  value: 42
  name: test
"#;
        let config: TestConfig = parse_config(yaml, ConfigFormat::Yaml).unwrap();
        assert_eq!(config.version, "1.0");
        assert_eq!(config.data.value, 42);
    }

    #[test]
    fn test_serialize_yaml() {
        let config = TestConfig {
            version: "1.0".to_string(),
            data: TestData {
                value: 42,
                name: "test".to_string(),
            },
        };
        let yaml = serialize_config(&config, ConfigFormat::Yaml).unwrap();
        // serde_yaml 可能不会对简单字符串加引号
        assert!(yaml.contains("version:"));
        assert!(yaml.contains("value: 42"));
    }
}
