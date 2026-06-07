// ============================================================================
// query_rules tool — 按需检索游戏规则
// ============================================================================
//
// LLM 在推理过程中按类别查询规则全文（从 PromptTemplate sections 渲染）。
// 配合 RuleCache 的分类索引使用：prompt 注入极简目录，LLM 自主决定查什么。

use crate::component::llm::tool_types::ToolDefinition;
use crate::component::rule_cache::RuleCache;
use cyber_jianghu_protocol::types::prompt_template::{PromptTemplateConfig, RuleCategoryConfig};

pub fn query_rules_definition(categories: &[RuleCategoryConfig]) -> ToolDefinition {
    let category_list = categories
        .iter()
        .map(|c| format!("{}({})", c.name, c.id))
        .collect::<Vec<_>>()
        .join("、");

    ToolDefinition::new(
        "query_rules",
        &format!(
            "查询游戏规则详情。可用类别: {}。在需要了解特定机制时调用。",
            category_list
        ),
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "categories": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "要查询的规则类别ID列表"
                }
            },
            "required": ["categories"]
        })),
    )
}

pub fn execute_query_rules(
    categories: &[String],
    rule_cache: &RuleCache,
    prompt_template: &PromptTemplateConfig,
) -> serde_json::Value {
    let results = rule_cache.query(categories, prompt_template);

    if results.is_empty() {
        return serde_json::json!({
            "success": false,
            "message": format!("未找到类别: {:?}", categories)
        });
    }

    serde_json::json!({
        "success": true,
        "rules": results.iter().map(|r| serde_json::json!({
            "category": r.id,
            "name": r.name,
            "content": r.content,
        })).collect::<Vec<_>>()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_jianghu_protocol::types::prompt_template::{RuleSectionsConfig, TemplateDef};
    use std::collections::HashMap;

    fn make_rule_config() -> RuleSectionsConfig {
        RuleSectionsConfig {
            enabled: true,
            categories: vec![
                RuleCategoryConfig {
                    id: "survival".into(),
                    name: "生存规则".into(),
                    description: "生存阈值".into(),
                    sections: vec!["survival_rules".into()],
                },
                RuleCategoryConfig {
                    id: "narrative".into(),
                    name: "叙事规则".into(),
                    description: "叙事限制".into(),
                    sections: vec!["narrative_limits".into()],
                },
                RuleCategoryConfig {
                    id: "empty".into(),
                    name: "空分类".into(),
                    description: "无对应section".into(),
                    sections: vec![],
                },
            ],
        }
    }

    fn make_prompt_template() -> PromptTemplateConfig {
        let mut sections = HashMap::new();
        sections.insert(
            "survival_rules".into(),
            "饥饿值超过80将严重损害健康。".into(),
        );
        sections.insert("narrative_limits".into(), "叙事应使用第二人称。".into());

        let mut templates = HashMap::new();
        templates.insert(
            "actor_direct".into(),
            TemplateDef {
                required_sections: vec![],
                sections,
                truncation: HashMap::new(),
                llm_parameters: HashMap::new(),
            },
        );

        PromptTemplateConfig {
            version: "test".into(),
            description: String::new(),
            templates,
            memory_narrative: None,
            rule_sections: None,
        }
    }

    #[test]
    fn test_execute_query_rules_normal() {
        let cache = RuleCache::new(&make_rule_config());
        let tmpl = make_prompt_template();
        let result = execute_query_rules(&["survival".into()], &cache, &tmpl);
        assert!(result["success"].as_bool().unwrap());
        let rules = result["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["category"], "survival");
        assert!(rules[0]["content"].as_str().unwrap().contains("饥饿"));
    }

    #[test]
    fn test_execute_query_rules_multiple() {
        let cache = RuleCache::new(&make_rule_config());
        let tmpl = make_prompt_template();
        let result = execute_query_rules(&["survival".into(), "narrative".into()], &cache, &tmpl);
        assert!(result["success"].as_bool().unwrap());
        assert_eq!(result["rules"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_execute_query_rules_unknown_category() {
        let cache = RuleCache::new(&make_rule_config());
        let tmpl = make_prompt_template();
        let result = execute_query_rules(&["nonexistent".into()], &cache, &tmpl);
        assert!(!result["success"].as_bool().unwrap());
        assert!(result["message"].as_str().unwrap().contains("nonexistent"));
    }

    #[test]
    fn test_execute_query_rules_empty_category() {
        let cache = RuleCache::new(&make_rule_config());
        let tmpl = make_prompt_template();
        let result = execute_query_rules(&["empty".into()], &cache, &tmpl);
        assert!(result["success"].as_bool().unwrap());
        let content = result["rules"].as_array().unwrap()[0]["content"]
            .as_str()
            .unwrap();
        assert!(content.contains("暂无"));
    }
}
