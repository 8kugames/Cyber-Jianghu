// ============================================================================
// Prompt 模板配置类型定义（共享）
// ============================================================================
//
// PromptTemplateConfig 在 protocol crate 定义，server 和 agent 共享。
// - Server: serde_yaml 解析 YAML → PromptTemplateConfig → serde_json 序列化为 JSON → WS 下发
// - Agent: serde_json 从 ConfigUpdate 反序列化 → PromptTemplateConfig
//
// 本文件只定义数据结构和 JSON 路径方法，不含 YAML 加载逻辑。
// ============================================================================

use std::collections::HashMap;

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// Prompt 模板配置顶层结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTemplateConfig {
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub templates: HashMap<String, TemplateDef>,
    /// 记忆叙事合成配置（独立于 templates，非标准模板结构）
    #[serde(default)]
    pub memory_narrative: Option<MemoryNarrativeConfig>,
}

/// 单个模板定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateDef {
    /// 必须存在的 section 列表（启动时校验）
    #[serde(default)]
    pub required_sections: Vec<String>,
    /// section 名 → 模板文本（含 {variable} 占位符）
    pub sections: HashMap<String, String>,
    /// 截断长度配置
    #[serde(default)]
    pub truncation: HashMap<String, usize>,
    /// LLM 调用参数配置（独立于 truncation）
    #[serde(default)]
    pub llm_parameters: HashMap<String, usize>,
}

/// 记忆叙事合成配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryNarrativeConfig {
    pub min_events: usize,
    #[serde(default = "default_max_events_per_tick")]
    pub max_events_per_tick: usize,
    pub max_narrative_len: usize,
    #[serde(default = "default_min_narrative_len")]
    pub min_narrative_len: usize,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    pub prompt: String,
}

fn default_max_events_per_tick() -> usize {
    10
}
fn default_min_narrative_len() -> usize {
    10
}
fn default_temperature() -> f32 {
    0.3
}

impl PromptTemplateConfig {
    /// 构造最小可用 fallback 配置（无模板时 Agent 仍可运行）
    pub fn default_fallback() -> Self {
        Self {
            version: "fallback-0.0".to_string(),
            description: String::new(),
            templates: HashMap::new(),
            memory_narrative: None,
        }
    }

    /// 从 JSON Value 构造（Server ConfigUpdate 下发路径）
    pub fn from_json_value(value: serde_json::Value) -> anyhow::Result<Self> {
        let config: Self =
            serde_json::from_value(value).context("JSON 反序列化 PromptTemplateConfig 失败")?;
        config.validate()?;
        Ok(config)
    }

    /// 序列化为 canonical JSON bytes（用于 SHA256 hash 计算）
    ///
    /// 两步序列化保证 key 排序确定性：
    /// 1. struct → serde_json::Value（HashMap entries 被收集到 Map=BTreeMap，自动排序）
    /// 2. Value → bytes（BTreeMap 迭代顺序稳定）
    pub fn to_json_bytes(&self) -> anyhow::Result<Vec<u8>> {
        let value =
            serde_json::to_value(self).context("PromptTemplateConfig → Value 序列化失败")?;
        serde_json::to_vec(&value).context("Value → bytes 序列化失败")
    }

    /// 校验所有模板的 required_sections
    pub fn validate(&self) -> anyhow::Result<()> {
        for (name, def) in &self.templates {
            for section in &def.required_sections {
                if !def.sections.contains_key(section) {
                    anyhow::bail!(
                        "Prompt 模板 '{}' 缺少 required_section: '{}'",
                        name,
                        section
                    );
                }
            }
        }
        Ok(())
    }

    /// 获取指定模板
    pub fn get_template(&self, name: &str) -> Option<&TemplateDef> {
        self.templates.get(name)
    }

    /// 获取截断长度配置
    pub fn truncation(&self, template_name: &str, key: &str, default: usize) -> usize {
        self.templates
            .get(template_name)
            .and_then(|t| t.truncation.get(key))
            .copied()
            .unwrap_or(default)
    }

    /// 获取 LLM 调用参数配置
    pub fn llm_param(&self, template_name: &str, key: &str, default: usize) -> usize {
        self.templates
            .get(template_name)
            .and_then(|t| t.llm_parameters.get(key))
            .copied()
            .unwrap_or(default)
    }

    /// 获取记忆叙事合成配置
    pub fn get_memory_narrative_config(&self) -> Option<&MemoryNarrativeConfig> {
        self.memory_narrative.as_ref()
    }

    /// 渲染记忆叙事合成 prompt
    pub fn render_memory_narrative(&self, vars: &HashMap<String, String>) -> Option<String> {
        let config = self.get_memory_narrative_config()?;
        let template = config.prompt.trim();
        let mut result = template.to_string();
        for (key, value) in vars {
            result = result.replace(&format!("{{{}}}", key), value);
        }
        Some(result)
    }
}

impl TemplateDef {
    /// 获取 section 内容，执行 {variable} 占位符替换
    pub fn render_section(&self, section: &str, vars: &HashMap<String, String>) -> Option<String> {
        let template = self.sections.get(section)?;
        let mut result = template.clone();
        for (key, value) in vars {
            result = result.replace(&format!("{{{}}}", key), value);
        }
        Some(result)
    }

    /// 按序渲染所有 section（required_sections 顺序 + 其余 section）
    pub fn render_all(&self, vars: &HashMap<String, String>) -> String {
        let mut parts = Vec::new();

        for section in &self.required_sections {
            if let Some(rendered) = self.render_section(section, vars) {
                parts.push(rendered);
            }
        }

        for name in self.sections.keys() {
            if !self.required_sections.contains(name)
                && let Some(rendered) = self.render_section(name, vars)
            {
                parts.push(rendered);
            }
        }

        parts.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_minimal_config() -> serde_json::Value {
        serde_json::json!({
            "version": "test-1.0",
            "templates": {
                "test_template": {
                    "required_sections": ["body"],
                    "sections": {
                        "body": "Hello {name}, you have {count} items."
                    },
                    "truncation": { "max_items": 3 }
                }
            }
        })
    }

    #[test]
    fn test_from_json_value_valid() {
        let config = PromptTemplateConfig::from_json_value(make_minimal_config());
        assert!(config.is_ok());
        let config = config.unwrap();
        assert_eq!(config.version, "test-1.0");
        assert!(config.templates.contains_key("test_template"));
    }

    #[test]
    fn test_from_json_value_missing_version() {
        let mut val = make_minimal_config();
        val.as_object_mut().unwrap().remove("version");
        let result = PromptTemplateConfig::from_json_value(val);
        assert!(result.is_err());
    }

    #[test]
    fn test_to_json_bytes_deterministic() {
        let config = PromptTemplateConfig::from_json_value(make_minimal_config()).unwrap();
        let bytes1 = config.to_json_bytes().unwrap();
        let bytes2 = config.to_json_bytes().unwrap();
        assert_eq!(
            bytes1, bytes2,
            "to_json_bytes must produce identical output"
        );
    }

    #[test]
    fn test_to_json_bytes_sorted_keys() {
        // 构造含多个模板的配置，验证 key 排序不影响输出
        let val = serde_json::json!({
            "version": "1.0",
            "templates": {
                "zebra": { "required_sections": ["x"], "sections": { "x": "z" } },
                "alpha": { "required_sections": ["x"], "sections": { "x": "a" } },
                "middle": { "required_sections": ["x"], "sections": { "x": "m" } }
            }
        });
        let config = PromptTemplateConfig::from_json_value(val).unwrap();
        let bytes = config.to_json_bytes().unwrap();
        let json_str = String::from_utf8(bytes).unwrap();
        // BTreeMap 排序: alpha < middle < zebra
        let alpha_pos = json_str.find("\"alpha\"").unwrap();
        let middle_pos = json_str.find("\"middle\"").unwrap();
        let zebra_pos = json_str.find("\"zebra\"").unwrap();
        assert!(
            alpha_pos < middle_pos && middle_pos < zebra_pos,
            "JSON keys must be sorted: alpha < middle < zebra"
        );
    }

    #[test]
    fn test_validate_passes() {
        let config = PromptTemplateConfig::from_json_value(make_minimal_config()).unwrap();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_missing_required_section() {
        // 直接构造 struct 绕过 from_json_value 的 validate 调用
        let config = PromptTemplateConfig {
            version: "1.0".to_string(),
            description: String::new(),
            templates: {
                let mut map = HashMap::new();
                map.insert(
                    "broken".to_string(),
                    TemplateDef {
                        required_sections: vec!["body".to_string(), "footer".to_string()],
                        sections: {
                            let mut s = HashMap::new();
                            s.insert("body".to_string(), "text".to_string());
                            s
                        },
                        truncation: HashMap::new(),
                        llm_parameters: HashMap::new(),
                    },
                );
                map
            },
            memory_narrative: None,
        };
        let result = config.validate();
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("footer"),
            "Error should mention missing section 'footer'"
        );
    }

    #[test]
    fn test_get_template() {
        let config = PromptTemplateConfig::from_json_value(make_minimal_config()).unwrap();
        let tmpl = config.get_template("test_template");
        assert!(tmpl.is_some());
        assert!(config.get_template("nonexistent").is_none());
    }

    #[test]
    fn test_truncation() {
        let config = PromptTemplateConfig::from_json_value(make_minimal_config()).unwrap();
        assert_eq!(config.truncation("test_template", "max_items", 99), 3);
        assert_eq!(config.truncation("test_template", "unknown_key", 42), 42);
        assert_eq!(config.truncation("nonexistent", "max_items", 7), 7);
    }

    #[test]
    fn test_render_section() {
        let config = PromptTemplateConfig::from_json_value(make_minimal_config()).unwrap();
        let tmpl = config.get_template("test_template").unwrap();
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "Alice".to_string());
        vars.insert("count".to_string(), "5".to_string());
        let rendered = tmpl.render_section("body", &vars).unwrap();
        assert_eq!(rendered, "Hello Alice, you have 5 items.");
    }

    #[test]
    fn test_render_section_missing_var_keeps_placeholder() {
        let config = PromptTemplateConfig::from_json_value(make_minimal_config()).unwrap();
        let tmpl = config.get_template("test_template").unwrap();
        let vars = HashMap::new();
        let rendered = tmpl.render_section("body", &vars).unwrap();
        assert!(
            rendered.contains("{name}"),
            "Unresolved vars should keep placeholder"
        );
    }

    #[test]
    fn test_from_json_value_with_memory_narrative() {
        let val = serde_json::json!({
            "version": "1.0",
            "templates": {},
            "memory_narrative": {
                "min_events": 3,
                "max_events_per_tick": 5,
                "max_narrative_len": 200,
                "min_narrative_len": 20,
                "temperature": 0.5,
                "prompt": "Summarize these events"
            }
        });
        let config = PromptTemplateConfig::from_json_value(val).unwrap();
        let mn = config.memory_narrative.as_ref().unwrap();
        assert_eq!(mn.min_events, 3);
        assert_eq!(mn.max_events_per_tick, 5);
        assert!((mn.temperature - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_from_json_value_empty_templates() {
        let val = serde_json::json!({
            "version": "1.0",
            "templates": {}
        });
        let config = PromptTemplateConfig::from_json_value(val).unwrap();
        assert!(config.templates.is_empty());
        assert!(config.memory_narrative.is_none());
    }
}
