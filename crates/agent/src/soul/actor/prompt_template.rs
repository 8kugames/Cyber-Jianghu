// ============================================================================
// Prompt 模板配置加载器
// ============================================================================
//
// 从 YAML 加载 prompt 模板，替代硬编码的 build_direct_prompt()。
// Fail-fast：缺失 required_sections 时启动即 panic。
// ============================================================================

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use serde::Deserialize;

// ============================================================================
// 数据结构
// ============================================================================

/// Prompt 模板配置顶层结构
#[derive(Debug, Clone, Deserialize)]
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
#[derive(Debug, Clone, Deserialize)]
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

// ============================================================================
// 加载与校验
// ============================================================================

impl PromptTemplateConfig {
    /// 从文件加载 prompt 模板配置
    pub fn load_from_file(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("读取 prompt 模板文件失败: {}", path.display()))?;
        Self::load_from_str(&content)
    }

    /// 从字符串加载 prompt 模板配置
    pub fn load_from_str(content: &str) -> anyhow::Result<Self> {
        let config: Self =
            serde_yaml::from_str(content).with_context(|| "解析 prompt 模板 YAML 失败")?;
        config.validate()?;
        Ok(config)
    }

    /// 校验所有模板的 required_sections
    fn validate(&self) -> anyhow::Result<()> {
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

/// 记忆叙事合成配置
#[derive(Debug, Clone, Deserialize)]
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

        // 先渲染 required_sections（保持顺序）
        for section in &self.required_sections {
            if let Some(rendered) = self.render_section(section, vars) {
                parts.push(rendered);
            }
        }

        // 再渲染其余 section（非 required 的）
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

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_and_validate() {
        let yaml = r#"
version: "0.0.1"
templates:
  actor_direct:
    required_sections:
      - header
      - task
    sections:
      header: "Hello {name}"
      task: "Do something {action}"
    truncation:
      max_len: 100
"#;
        let config = PromptTemplateConfig::load_from_str(yaml).unwrap();
        assert_eq!(config.templates.len(), 1);

        let tmpl = config.get_template("actor_direct").unwrap();
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "World".to_string());
        vars.insert("action".to_string(), "now".to_string());

        let rendered = tmpl.render_section("header", &vars).unwrap();
        assert_eq!(rendered, "Hello World");
    }

    #[test]
    fn test_validate_missing_section() {
        let yaml = r#"
version: "0.0.1"
templates:
  actor_direct:
    required_sections:
      - header
      - missing_section
    sections:
      header: "Hello"
"#;
        let result = PromptTemplateConfig::load_from_str(yaml);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("missing_section"));
    }

    #[test]
    fn test_truncation_config() {
        let yaml = r#"
version: "0.0.1"
templates:
  actor_direct:
    required_sections: []
    sections: {}
    truncation:
      planning_description: 100
      content_hint: 30
"#;
        let config = PromptTemplateConfig::load_from_str(yaml).unwrap();
        assert_eq!(
            config.truncation("actor_direct", "planning_description", 50),
            100
        );
        assert_eq!(config.truncation("actor_direct", "nonexistent", 50), 50);
    }
}
