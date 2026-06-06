// ============================================================================
// 规则缓存 — EarthSoul query_rules tool 的数据源
// ============================================================================
//
// 规则文本存储在 PromptTemplateConfig 的 sections 中（survival_rules,
// narrative_limits 等）。RuleCache 持有分类配置，按需从 PromptTemplate 渲染。
//
// 不缓存规则全文：PromptTemplate 热更新时内容自动跟随，零同步逻辑。

use cyber_jianghu_protocol::types::prompt_template::{
    PromptTemplateConfig, RuleCategoryConfig, RuleSectionsConfig,
};
use std::collections::HashMap;

/// 查询结果：一个分类的规则内容
#[derive(Debug, Clone)]
pub struct RuleCategoryContent {
    pub id: String,
    pub name: String,
    pub content: String,
    pub token_estimate: u32,
}

/// 规则缓存：持有分类配置和索引
#[derive(Clone)]
pub struct RuleCache {
    categories: Vec<RuleCategoryConfig>,
    category_index: HashMap<String, usize>,
    index_summary: String,
}

impl RuleCache {
    /// 从 PromptTemplateConfig.rule_sections 构建
    pub fn new(config: &RuleSectionsConfig) -> Self {
        let category_index: HashMap<String, usize> = config
            .categories
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id.clone(), i))
            .collect();

        let index_summary = {
            let list: String = config
                .categories
                .iter()
                .map(|c| format!("{}({})", c.name, c.id))
                .collect::<Vec<_>>()
                .join("、");
            format!("可查询的规则类别: {}。使用 query_rules 工具查询详情。", list)
        };

        Self {
            categories: config.categories.clone(),
            category_index,
            index_summary,
        }
    }

    /// 按类别 ID 查询规则内容
    pub fn query(
        &self,
        category_ids: &[String],
        prompt_template: &PromptTemplateConfig,
    ) -> Vec<RuleCategoryContent> {
        let mut results = Vec::new();
        for id in category_ids {
            let Some(idx) = self.category_index.get(id) else {
                continue;
            };
            let cat = &self.categories[*idx];
            let mut content_parts = Vec::new();

            for section_key in &cat.sections {
                let empty_vars = HashMap::new();
                if let Some(rendered) = prompt_template
                    .get_template("actor_direct")
                    .and_then(|t| t.render_section(section_key, &empty_vars))
                {
                    content_parts.push(rendered);
                }
            }

            let content = if content_parts.is_empty() {
                format!("暂无「{}」的详细规则。", cat.name)
            } else {
                content_parts.join("\n\n")
            };

            let token_estimate = (content.chars().count() as f64 / 4.0).ceil() as u32;

            results.push(RuleCategoryContent {
                id: cat.id.clone(),
                name: cat.name.clone(),
                token_estimate,
                content,
            });
        }
        results
    }

    pub fn index_summary(&self) -> &str {
        &self.index_summary
    }

    pub fn categories(&self) -> &[RuleCategoryConfig] {
        &self.categories
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_jianghu_protocol::types::prompt_template::TemplateDef;

    fn make_rule_config() -> RuleSectionsConfig {
        RuleSectionsConfig {
            enabled: true,
            categories: vec![
                RuleCategoryConfig {
                    id: "survival".into(),
                    name: "生存规则".into(),
                    description: "饥饿、健康阈值".into(),
                    sections: vec!["survival_rules".into()],
                },
                RuleCategoryConfig {
                    id: "narrative".into(),
                    name: "叙事规则".into(),
                    description: "叙事限制".into(),
                    sections: vec!["narrative_limits".into()],
                },
                RuleCategoryConfig {
                    id: "combat".into(),
                    name: "战斗规则".into(),
                    description: "战斗机制".into(),
                    sections: vec![],
                },
            ],
        }
    }

    fn make_prompt_template() -> PromptTemplateConfig {
        let mut sections = HashMap::new();
        sections.insert(
            "survival_rules".into(),
            "饥饿值超过80将严重损害健康。健康归零即死亡。".into(),
        );
        sections.insert(
            "narrative_limits".into(),
            "叙事应使用第二人称。禁止元叙事。".into(),
        );

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
    fn test_index_summary_contains_all_categories() {
        let cache = RuleCache::new(&make_rule_config());
        let summary = cache.index_summary();
        assert!(summary.contains("生存规则(survival)"));
        assert!(summary.contains("叙事规则(narrative)"));
        assert!(summary.contains("战斗规则(combat)"));
    }

    #[test]
    fn test_query_existing_category() {
        let cache = RuleCache::new(&make_rule_config());
        let tmpl = make_prompt_template();
        let results = cache.query(&["survival".into()], &tmpl);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "survival");
        assert!(results[0].content.contains("饥饿"));
    }

    #[test]
    fn test_query_empty_category() {
        let cache = RuleCache::new(&make_rule_config());
        let tmpl = make_prompt_template();
        let results = cache.query(&["combat".into()], &tmpl);
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("暂无"));
    }

    #[test]
    fn test_query_unknown_category_returns_empty() {
        let cache = RuleCache::new(&make_rule_config());
        let tmpl = make_prompt_template();
        let results = cache.query(&["nonexistent".into()], &tmpl);
        assert!(results.is_empty());
    }

    #[test]
    fn test_query_multiple_categories() {
        let cache = RuleCache::new(&make_rule_config());
        let tmpl = make_prompt_template();
        let results = cache.query(
            &["survival".into(), "narrative".into()],
            &tmpl,
        );
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_token_estimate_uses_chars() {
        let cache = RuleCache::new(&make_rule_config());
        let tmpl = make_prompt_template();
        let results = cache.query(&["survival".into()], &tmpl);
        // "饥饿值超过80将严重损害健康。健康归零即死亡。" = 23 chars → 23/4 = 5.75 → ceil = 6
        assert_eq!(results[0].token_estimate, 6);
    }
}
