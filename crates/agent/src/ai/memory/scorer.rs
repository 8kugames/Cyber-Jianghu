// ============================================================================
// 重要性评分器
// ============================================================================
// 设计文档: (项目根)/docs/superpowers/specs/2025-03-15-semantic-memory-and-forgetting-design.md
//
// 基于事件内容和类型计算重要性评分
// 用于决定哪些事件值得长期存储
// ============================================================================
use serde_json::Value;

/// 重要性评分器
pub struct ImportanceScorer;

impl ImportanceScorer {
    /// 创建新的评分器
    pub fn new() -> Self {
        Self
    }

    /// 计算事件重要性评分（0.0-1.0）
    ///
    /// 评分规则：
    /// - 1.0: 最高重要性（死亡、濒死等）
    /// - 0.8-0.9: 高重要性（战斗、高价值交易）
    /// - 0.5-0.7: 中等重要性（环境伤害、社交互动）
    /// - 0.2-0.4: 低重要性（日常动作）
    pub fn score(&self, event_type: &str, description: &str, metadata: &Value) -> f32 {
        // 基础评分（基于事件类型）
        let base_score = self.score_by_type(event_type);

        // 内容特征调整
        let content_adjustment = self.score_by_content(description);

        // 元数据调整（基于事件的具体数值）
        let metadata_adjustment = self.score_by_metadata(metadata);

        // 综合评分（限制在 0.0-1.0 范围内）
        (base_score + content_adjustment + metadata_adjustment).clamp(0.0, 1.0)
    }

    /// 基于事件类型的评分
    fn score_by_type(&self, event_type: &str) -> f32 {
        match event_type {
            "agent_death" => 1.0,
            "near_death" => 0.95,
            "combat" => 0.8,
            "high_value_trade" => 0.75,
            "environmental_damage" => 0.6,
            "social_interaction" => 0.5,
            "trade" => 0.4,
            "item_use" => 0.3,
            "routine" => 0.2,
            _ => 0.3,
        }
    }

    /// 基于内容的评分调整
    fn score_by_content(&self, description: &str) -> f32 {
        let content_lower = description.to_lowercase();
        let mut adjustment: f32 = 0.0;

        // 关键词检测
        let critical_keywords = ["死亡", "重伤", "濒死", "致命"];
        let high_keywords = ["战斗", "攻击", "偷窃", "抢劫", "银两", "交易"];
        let medium_keywords = ["伤害", "饥饿", "口渴", "疲劳"];

        for keyword in &critical_keywords {
            if content_lower.contains(keyword) {
                adjustment += 0.2;
                break;
            }
        }

        for keyword in &high_keywords {
            if content_lower.contains(keyword) {
                adjustment += 0.1;
            }
        }

        for keyword in &medium_keywords {
            if content_lower.contains(keyword) {
                adjustment += 0.05;
            }
        }

        adjustment.min(0.3) // 限制最大调整幅度
    }

    /// 基于元数据的评分调整
    fn score_by_metadata(&self, metadata: &Value) -> f32 {
        let mut adjustment: f32 = 0.0;

        // 检查 HP 变化
        if let Some(hp_delta) = metadata.get("hp_delta").and_then(|v| v.as_i64()) {
            if hp_delta < -20 {
                adjustment += 0.2; // 大量伤害
            } else if hp_delta < -10 {
                adjustment += 0.1;
            } else if hp_delta > 10 {
                adjustment += 0.05; // 恢复 HP
            }
        }

        // 检查交易金额
        if let Some(amount) = metadata.get("trade_amount").and_then(|v| v.as_i64()) {
            if amount > 50 {
                adjustment += 0.2; // 高价值交易
            } else if amount > 20 {
                adjustment += 0.1;
            }
        }

        // 检查是否涉及稀有物品
        if let Some(items) = metadata.get("items").and_then(|v| v.as_array()) {
            for item in items {
                if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                    if name.contains("刀") || name.contains("剑") || name.contains("秘籍") {
                        adjustment += 0.15;
                    }
                }
            }
        }

        adjustment.min(0.3) // 限制最大调整幅度
    }
}

impl Default for ImportanceScorer {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_score_by_type() {
        let scorer = ImportanceScorer::new();

        assert_eq!(scorer.score_by_type("agent_death"), 1.0);
        assert_eq!(scorer.score_by_type("combat"), 0.8);
        assert_eq!(scorer.score_by_type("social_interaction"), 0.5);
        assert_eq!(scorer.score_by_type("routine"), 0.2);
    }

    #[test]
    fn test_score_by_content() {
        let scorer = ImportanceScorer::new();

        let death_score = scorer.score_by_content("你因饥饿而死亡");
        assert!(death_score > 0.1);

        let combat_score = scorer.score_by_content("你受到了10点伤害");
        assert!(combat_score > 0.0);

        let normal_score = scorer.score_by_content("你休息了一会");
        assert!(normal_score < 0.1);
    }

    #[test]
    fn test_score_by_metadata() {
        let scorer = ImportanceScorer::new();

        let high_damage = json!({"hp_delta": -30});
        assert!(scorer.score_by_metadata(&high_damage) > 0.15);

        let high_trade = json!({"trade_amount": 100});
        assert!(scorer.score_by_metadata(&high_trade) > 0.15);
    }

    #[test]
    fn test_full_scoring() {
        let scorer = ImportanceScorer::new();

        // 战斗事件 + 高伤害
        let metadata = json!({"hp_delta": -25});
        let score = scorer.score("combat", "你受到了25点伤害，濒临死亡", &metadata);
        assert!(score > 0.8);

        // 普通休息
        let metadata = json!({});
        let score = scorer.score("routine", "你休息了一会", &metadata);
        assert!(score < 0.4);
    }
}
