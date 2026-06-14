use std::collections::HashMap;

use cyber_jianghu_protocol::types::governance::GovernanceTopic;

use super::types::{ClassificationResult, TopicClassifierConfig};

pub struct TopicClassifier {
    config: TopicClassifierConfig,
}

impl TopicClassifier {
    pub fn new(config: TopicClassifierConfig) -> Self {
        Self { config }
    }

    /// 基于 effect_refs 做规则匹配
    ///
    /// Phase 0：agent 提议时 effect_refs 为空（agent 无可信数据源），
    /// 走 fallback topic = evolution → 伏羲路由。Phase 2 多 soul 上线时
    /// 由 LLM 推断后回填 effect_refs 做实际分类。
    pub fn classify(
        &self,
        effect_refs: &[String],
        agent_topics: &[GovernanceTopic],
        agent_confidence: &HashMap<GovernanceTopic, f64>,
    ) -> ClassificationResult {
        let threshold = self.config.confidence_threshold;

        // If agent provides topics and all are confident enough, trust them
        if !agent_topics.is_empty() {
            let all_confident = agent_topics
                .iter()
                .all(|t| agent_confidence.get(t).copied().unwrap_or(0.0) >= threshold);
            if all_confident {
                return ClassificationResult {
                    topics: agent_topics.to_vec(),
                    confidence: agent_confidence.clone(),
                    fallback_used: false,
                };
            }
        }

        // Rule-based matching on effect_refs
        let mut matched_topics: Vec<GovernanceTopic> = Vec::new();
        for effect_ref in effect_refs {
            for rule in &self.config.rules {
                for prefix in &rule.matcher.effect_refs_prefix {
                    if effect_ref.starts_with(prefix) {
                        for topic in &rule.topics {
                            if !matched_topics.contains(topic) {
                                matched_topics.push(*topic);
                            }
                        }
                    }
                }
            }
        }

        if !matched_topics.is_empty() {
            let confidence: HashMap<GovernanceTopic, f64> = matched_topics
                .iter()
                .map(|t| (*t, self.config.confidence_threshold))
                .collect();
            return ClassificationResult {
                topics: matched_topics,
                confidence,
                fallback_used: false,
            };
        }

        // Fallback to default topic
        let fallback = match self.config.default_fallback_topic.as_str() {
            "resource" => GovernanceTopic::Resource,
            "order" => GovernanceTopic::Order,
            _ => GovernanceTopic::Evolution,
        };
        ClassificationResult {
            topics: vec![fallback],
            confidence: [(fallback, self.config.fallback_confidence)]
                .into_iter()
                .collect(),
            fallback_used: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> TopicClassifierConfig {
        TopicClassifierConfig {
            rules: vec![super::super::types::TopicClassifierRule {
                matcher: super::super::types::TopicClassifierMatch {
                    effect_refs_prefix: vec!["combat".to_string()],
                    effect_refs_any: vec![],
                },
                topics: vec![GovernanceTopic::Order],
            }],
            confidence_threshold: 0.6,
            default_fallback_topic: "evolution".to_string(),
            fallback_confidence: 0.5,
        }
    }

    #[test]
    fn test_classify_fallback() {
        let classifier = TopicClassifier::new(test_config());
        let result = classifier.classify(&[], &[], &HashMap::new());
        assert!(result.fallback_used);
        assert_eq!(result.topics, vec![GovernanceTopic::Evolution]);
    }

    #[test]
    fn test_classify_rule_match() {
        let classifier = TopicClassifier::new(test_config());
        let effect_refs = vec!["combat.slash".to_string()];
        let result = classifier.classify(&effect_refs, &[], &HashMap::new());
        assert!(!result.fallback_used);
        assert!(result.topics.contains(&GovernanceTopic::Order));
    }

    #[test]
    fn test_classify_agent_topics_trusted() {
        let classifier = TopicClassifier::new(test_config());
        let agent_topics = vec![GovernanceTopic::Resource];
        let confidence: HashMap<GovernanceTopic, f64> =
            [(GovernanceTopic::Resource, 0.8)].into_iter().collect();
        let result = classifier.classify(&[], &agent_topics, &confidence);
        assert!(!result.fallback_used);
        assert_eq!(result.topics, vec![GovernanceTopic::Resource]);
    }
}
