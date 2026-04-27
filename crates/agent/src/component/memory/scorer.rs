// ============================================================================
// 重要性评分器
// ============================================================================
// 设计文档: (项目根)/docs/superpowers/specs/2025-03-15-semantic-memory-and-forgetting-design.md
//
// 基于事件内容和类型计算重要性评分
// 用于决定哪些事件值得长期存储
// ============================================================================
use crate::models::WorldEventType;
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
    /// - 1.0: 最高重要性（死亡通知）
    /// - 0.8: 高重要性（状态变更）
    /// - 0.7: 社交核心（密语、公开说话、社交互动）
    /// - 0.4: 中等重要性（动作结果）
    /// - 0.2: 低重要性（环境变化、系统通知）
    /// - 0.1: 最低重要性（时间更新）
    pub fn score(&self, event_type: &WorldEventType, description: &str, metadata: &Value) -> f32 {
        let base_score = self.score_by_type(event_type);
        let content_adjustment = self.score_by_content(description);
        let metadata_adjustment = self.score_by_metadata(metadata);
        (base_score + content_adjustment + metadata_adjustment).clamp(0.0, 1.0)
    }

    /// 基于事件类型的评分
    fn score_by_type(&self, event_type: &WorldEventType) -> f32 {
        match event_type {
            WorldEventType::DeathNotification => 1.0,
            WorldEventType::StateChange => 0.8,
            WorldEventType::PrivateDialogue => 0.7,
            WorldEventType::PublicMessage => 0.7,
            WorldEventType::SocialInteraction => 0.7,
            WorldEventType::ActionResult => 0.4,
            WorldEventType::EnvironmentalChange => 0.2,
            WorldEventType::SystemNotification => 0.2,
            WorldEventType::TimeUpdate => 0.1,
        }
    }

    /// 基于内容的评分调整
    fn score_by_content(&self, description: &str) -> f32 {
        let content_lower = description.to_lowercase();
        let mut adjustment: f32 = 0.0;

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

        adjustment.min(0.3)
    }

    /// 基于元数据的评分调整
    fn score_by_metadata(&self, metadata: &Value) -> f32 {
        let mut adjustment: f32 = 0.0;

        if let Some(hp_delta) = metadata.get("hp_delta").and_then(|v| v.as_i64()) {
            if hp_delta < -20 {
                adjustment += 0.2;
            } else if hp_delta < -10 {
                adjustment += 0.1;
            } else if hp_delta > 10 {
                adjustment += 0.05;
            }
        }

        if let Some(amount) = metadata.get("trade_amount").and_then(|v| v.as_i64()) {
            if amount > 50 {
                adjustment += 0.2;
            } else if amount > 20 {
                adjustment += 0.1;
            }
        }

        if let Some(items) = metadata.get("items").and_then(|v| v.as_array()) {
            for item in items {
                if let Some(name) = item.get("name").and_then(|v| v.as_str())
                    && (name.contains("刀") || name.contains("剑") || name.contains("秘籍"))
                {
                    adjustment += 0.15;
                }
            }
        }

        adjustment.min(0.3)
    }
}

impl Default for ImportanceScorer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_score_by_type() {
        let scorer = ImportanceScorer::new();
        assert_eq!(
            scorer.score_by_type(&WorldEventType::DeathNotification),
            1.0
        );
        assert_eq!(scorer.score_by_type(&WorldEventType::ActionResult), 0.4);
        assert_eq!(
            scorer.score_by_type(&WorldEventType::SocialInteraction),
            0.7
        );
        assert_eq!(scorer.score_by_type(&WorldEventType::PublicMessage), 0.7);
        assert_eq!(scorer.score_by_type(&WorldEventType::PrivateDialogue), 0.7);
        assert_eq!(
            scorer.score_by_type(&WorldEventType::EnvironmentalChange),
            0.2
        );
        assert_eq!(scorer.score_by_type(&WorldEventType::TimeUpdate), 0.1);
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

        // 死亡通知 + 高伤害关键词
        let metadata = json!({"hp_delta": -25});
        let score = scorer.score(
            &WorldEventType::DeathNotification,
            "你受到了25点伤害，濒临死亡",
            &metadata,
        );
        assert!(score > 0.8);

        // 普通动作结果
        let metadata = json!({});
        let score = scorer.score(&WorldEventType::ActionResult, "你休息了一会", &metadata);
        assert!(score < 0.5);
    }
}
