// ============================================================================
// Prompt 模板配置加载器
// ============================================================================
//
// PromptTemplateConfig 定义在 protocol crate（server/agent 共享）。
// 本文件提供本地 JSON 文件加载路径（agent 启动时 fallback）。
// WS ConfigUpdate 路径直接使用 PromptTemplateConfig::from_json_value()。
//
// 统一使用 JSON 格式：Server 端 YAML→JSON 转换后下发 JSON，
// Agent 端本地加载也使用 JSON，消除 serde_yaml 跨平台解析问题。
// ============================================================================

use std::path::Path;

use anyhow::Context;

pub use cyber_jianghu_protocol::{MemoryNarrativeConfig, PromptTemplateConfig, TemplateDef};

/// 空壳 fallback 配置的 version 标识（load 和 save 共用，避免魔法字符串）
pub const EMPTY_FALLBACK_VERSION: &str = "empty-fallback";

/// 从文件加载 prompt 模板配置（本地 JSON fallback）
pub fn load_prompt_template_from_file(path: &Path) -> anyhow::Result<PromptTemplateConfig> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("读取 prompt 模板文件失败: {}", path.display()))?;
    load_prompt_template_from_str(&content)
}

/// 从字符串加载 prompt 模板配置（本地 JSON fallback）
pub fn load_prompt_template_from_str(content: &str) -> anyhow::Result<PromptTemplateConfig> {
    let config: PromptTemplateConfig =
        serde_json::from_str(content).with_context(|| "解析 prompt 模板 JSON 失败")?;
    config.validate()?;
    Ok(config)
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_and_validate() {
        let json = r#"{"version":"0.0.1","templates":{"actor_direct":{"required_sections":["header","task"],"sections":{"header":"Hello {name}","task":"Do something {action}"},"truncation":{"max_len":100}}}}"#;
        let config = load_prompt_template_from_str(json).unwrap();
        assert_eq!(config.templates.len(), 1);

        let tmpl = config.get_template("actor_direct").unwrap();
        let mut vars = std::collections::HashMap::new();
        vars.insert("name".to_string(), "World".to_string());
        vars.insert("action".to_string(), "now".to_string());

        let rendered = tmpl.render_section("header", &vars).unwrap();
        assert_eq!(rendered, "Hello World");
    }

    #[test]
    fn test_validate_missing_section() {
        let json = r#"{"version":"0.0.1","templates":{"actor_direct":{"required_sections":["header","missing_section"],"sections":{"header":"Hello"}}}}"#;
        let result = load_prompt_template_from_str(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("missing_section"));
    }

    #[test]
    fn test_truncation_config() {
        let json = r#"{"version":"0.0.1","templates":{"actor_direct":{"required_sections":[],"sections":{},"truncation":{"planning_description":100,"content_hint":30}}}}"#;
        let config = load_prompt_template_from_str(json).unwrap();
        assert_eq!(
            config.truncation("actor_direct", "planning_description", 50),
            100
        );
        assert_eq!(config.truncation("actor_direct", "nonexistent", 50), 50);
    }

    #[test]
    fn test_from_json_value() {
        let json = r#"{"version":"0.0.1","templates":{"actor_direct":{"required_sections":["header"],"sections":{"header":"Test"},"truncation":{"max_len":50}}}}"#;
        let config = load_prompt_template_from_str(json).unwrap();
        let json_val = serde_json::to_value(&config).unwrap();
        let parsed = PromptTemplateConfig::from_json_value(json_val).unwrap();
        assert_eq!(parsed.version, "0.0.1");
        assert_eq!(parsed.truncation("actor_direct", "max_len", 0), 50);
    }

    #[test]
    fn test_to_json_bytes_deterministic() {
        let json = r#"{"version":"0.0.1","templates":{"actor_direct":{"required_sections":[],"sections":{}}}}"#;
        let config = load_prompt_template_from_str(json).unwrap();
        let bytes1 = config.to_json_bytes().unwrap();
        let bytes2 = config.to_json_bytes().unwrap();
        assert_eq!(bytes1, bytes2, "to_json_bytes must be deterministic");
    }
}
