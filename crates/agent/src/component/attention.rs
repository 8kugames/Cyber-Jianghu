// ============================================================================
// Attention Controller -- 规则过滤 + 轻量 LLM 排序
// 两阶段架构: Phase 1 (规则过滤, 零 token) + Phase 2 (可选轻量 LLM 排序)
// ============================================================================

use crate::component::delta_engine::{ChangeCategory, StateChange, StateDelta, Urgency};
use crate::config::AttentionConfig;
use std::collections::HashSet;

/// Focus summary 条目 (过滤/排序后)
#[derive(Debug, Clone)]
pub struct FocusItem {
    pub change: StateChange,
    pub rank: usize, // 0 = 最高优先级
}

/// 最终 Focus Summary (注入 prompt)
#[derive(Debug, Clone)]
pub struct FocusSummary {
    pub items: Vec<FocusItem>,
    pub narrative: String, // 带工具提示的格式化文本
    pub is_first_tick: bool,
}

/// Attention Controller: 过滤和排序状态变化
pub struct AttentionController {
    config: AttentionConfig,
    social_targets: HashSet<String>, // 关心的 agent 名称
}

impl AttentionController {
    pub fn new(config: AttentionConfig) -> Self {
        Self {
            config,
            social_targets: HashSet::new(),
        }
    }

    /// 更新社交目标 (从 RelationshipStore 数据调用)
    pub fn set_social_targets(&mut self, targets: HashSet<String>) {
        self.social_targets = targets;
    }

    /// 主入口: 将 delta 过滤为 focus summary
    pub fn filter(&self, delta: &StateDelta) -> FocusSummary {
        let max_items = if delta.is_first_tick {
            self.config.first_tick_focus_cap
        } else {
            self.config.max_focus_items
        };

        let (auto_focus, candidates) = self.phase1_rule_filter(&delta.changes);

        // auto_focus 优先，再用 candidates 填充
        let mut selected: Vec<FocusItem> = auto_focus
            .into_iter()
            .enumerate()
            .map(|(i, change)| FocusItem {
                change,
                rank: i,
            })
            .collect();

        let remaining_slots = max_items.saturating_sub(selected.len());

        if remaining_slots > 0 && !candidates.is_empty() {
            // TODO: Phase 2 LLM ranking will be called from lifecycle with LLM client access
            // For now, candidates are taken in original order (newest first from Delta Engine)
            for (i, change) in candidates.into_iter().take(remaining_slots).enumerate() {
                selected.push(FocusItem {
                    change,
                    rank: selected.len() + i,
                });
            }
        }

        let narrative = self.generate_narrative(&selected);

        FocusSummary {
            items: selected,
            narrative,
            is_first_tick: delta.is_first_tick,
        }
    }

    /// Phase 1: 规则过滤 (零 token)
    /// 返回 (auto_focus, candidates)
    fn phase1_rule_filter(
        &self,
        changes: &[StateChange],
    ) -> (Vec<StateChange>, Vec<StateChange>) {
        let mut auto_focus = Vec::new();
        let mut candidates = Vec::new();

        for change in changes {
            let should_auto = match (&change.urgency, &change.category) {
                // Critical 由配置决定是否自动包含
                (Urgency::Critical, _) => self.config.critical_auto_include,
                // Important + Survival -> 自动
                (Urgency::Important, ChangeCategory::Survival) => true,
                // Important + Social + 在社交目标中 -> 自动
                (Urgency::Important, ChangeCategory::Social)
                    if self.is_social_target(&change.description) => true,
                // 其余 -> candidate
                _ => false,
            };

            if should_auto {
                auto_focus.push(change.clone());
            } else {
                candidates.push(change.clone());
            }
        }

        (auto_focus, candidates)
    }

    /// 检查变化是否涉及社交目标
    fn is_social_target(&self, description: &str) -> bool {
        self.social_targets
            .iter()
            .any(|t| description.contains(t))
    }

    /// 生成带工具提示的叙述摘要
    fn generate_narrative(&self, items: &[FocusItem]) -> String {
        if items.is_empty() {
            return "无显著变化。".to_string();
        }

        let mut lines = Vec::new();

        for item in items {
            let urgency_tag = match item.change.urgency {
                Urgency::Critical => "[紧迫]",
                Urgency::Important => "[变化]",
                Urgency::Info => "[信息]",
            };

            let tool_hint_suffix = match &item.change.tool_hint {
                Some(hint) => format!(" (查询: {})", hint),
                None => String::new(),
            };

            lines.push(format!(
                "{} {}{}",
                urgency_tag, item.change.description, tool_hint_suffix
            ));
        }

        lines.join("\n")
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::delta_engine::{ChangeCategory, StateChange, StateDelta, Urgency};
    use crate::config::AttentionConfig;
    use std::collections::HashSet;

    fn default_config() -> AttentionConfig {
        AttentionConfig {
            max_focus_items: 5,
            first_tick_focus_cap: 15,
            critical_auto_include: true,
            enable_llm_ranking: true,
            llm_ranking_model: "haiku".to_string(),
        }
    }

    fn make_change(
        category: ChangeCategory,
        urgency: Urgency,
        description: &str,
        tool_hint: Option<&str>,
    ) -> StateChange {
        StateChange {
            category,
            urgency,
            field: "test_field".to_string(),
            description: description.to_string(),
            data: serde_json::json!({}),
            tool_hint: tool_hint.map(|s| s.to_string()),
        }
    }

    fn make_delta(changes: Vec<StateChange>, is_first_tick: bool) -> StateDelta {
        StateDelta {
            changes,
            is_first_tick,
        }
    }

    #[test]
    fn test_critical_auto_included() {
        let ctrl = AttentionController::new(default_config());
        let changes = vec![
            make_change(
                ChangeCategory::Survival,
                Urgency::Critical,
                "hp: 50 -> 20",
                None,
            ),
            make_change(
                ChangeCategory::Environment,
                Urgency::Info,
                "天气变化",
                None,
            ),
        ];
        let delta = make_delta(changes, false);
        let summary = ctrl.filter(&delta);

        assert_eq!(summary.items.len(), 2);
        assert_eq!(summary.items[0].change.urgency, Urgency::Critical);
        assert_eq!(summary.items[0].rank, 0);
    }

    #[test]
    fn test_important_survival_auto_included() {
        let ctrl = AttentionController::new(default_config());
        let changes = vec![
            make_change(
                ChangeCategory::Survival,
                Urgency::Important,
                "hunger: 50 -> 30",
                Some("query_world(section=inventory, filter=food)"),
            ),
            make_change(
                ChangeCategory::Environment,
                Urgency::Info,
                "无足轻重",
                None,
            ),
        ];
        let delta = make_delta(changes, false);
        let summary = ctrl.filter(&delta);

        // Important + Survival = auto, Info = candidate
        assert!(!summary.items.is_empty());
        assert_eq!(summary.items[0].change.category, ChangeCategory::Survival);
    }

    #[test]
    fn test_social_target_included() {
        let config = default_config();
        let mut ctrl = AttentionController::new(config);
        ctrl.set_social_targets(HashSet::from(["张三".to_string()]));

        let changes = vec![
            make_change(
                ChangeCategory::Social,
                Urgency::Important,
                "张三 出现",
                Some("query_world(section=entities, filter=张三)"),
            ),
            make_change(
                ChangeCategory::Social,
                Urgency::Important,
                "李四 出现",
                None,
            ),
        ];
        let delta = make_delta(changes, false);
        let summary = ctrl.filter(&delta);

        // "张三 出现" 应在 auto_focus (排前面), "李四 出现" 在 candidate
        assert!(!summary.items.is_empty());
        assert!(summary.items[0].change.description.contains("张三"));
    }

    #[test]
    fn test_non_target_social_is_candidate() {
        let mut ctrl = AttentionController::new(default_config());
        ctrl.set_social_targets(HashSet::from(["张三".to_string()]));

        let changes = vec![
            make_change(
                ChangeCategory::Social,
                Urgency::Important,
                "李四 出现",
                None,
            ),
        ];
        let delta = make_delta(changes, false);
        let summary = ctrl.filter(&delta);

        // "李四 出现" 不是社交目标，应作为 candidate (在 auto_focus 之后)
        assert_eq!(summary.items.len(), 1);
        assert_eq!(summary.items[0].change.description, "李四 出现");
    }

    #[test]
    fn test_max_focus_items_limit() {
        let config = AttentionConfig {
            max_focus_items: 2,
            ..default_config()
        };
        let ctrl = AttentionController::new(config);

        let changes: Vec<StateChange> = (0..10)
            .map(|i| {
                make_change(
                    ChangeCategory::Environment,
                    Urgency::Info,
                    &format!("事件 {}", i),
                    None,
                )
            })
            .collect();
        let delta = make_delta(changes, false);
        let summary = ctrl.filter(&delta);

        // 非 first_tick，max_focus_items = 2
        assert!(summary.items.len() <= 2);
    }

    #[test]
    fn test_first_tick_higher_cap() {
        let config = AttentionConfig {
            max_focus_items: 2,
            first_tick_focus_cap: 10,
            ..default_config()
        };
        let ctrl = AttentionController::new(config);

        let changes: Vec<StateChange> = (0..8)
            .map(|i| {
                make_change(
                    ChangeCategory::Environment,
                    Urgency::Info,
                    &format!("初始事件 {}", i),
                    None,
                )
            })
            .collect();
        let delta = make_delta(changes, true);
        let summary = ctrl.filter(&delta);

        // first_tick, cap = 10, 有 8 个变化
        assert_eq!(summary.items.len(), 8);
        assert!(summary.is_first_tick);
    }

    #[test]
    fn test_empty_delta() {
        let ctrl = AttentionController::new(default_config());
        let delta = make_delta(vec![], false);
        let summary = ctrl.filter(&delta);

        assert!(summary.items.is_empty());
        assert_eq!(summary.narrative, "无显著变化。");
    }

    #[test]
    fn test_narrative_format() {
        let ctrl = AttentionController::new(default_config());
        let changes = vec![
            make_change(
                ChangeCategory::Survival,
                Urgency::Critical,
                "hp: 50 -> 20",
                Some("query_world(section=state)"),
            ),
            make_change(
                ChangeCategory::Environment,
                Urgency::Info,
                "天气晴朗",
                None,
            ),
        ];
        let delta = make_delta(changes, false);
        let summary = ctrl.filter(&delta);

        // 应包含紧急标签
        assert!(summary.narrative.contains("[紧迫]"));
        // 应包含工具提示
        assert!(summary.narrative.contains("查询: query_world(section=state)"));
        // Info 级别的描述
        assert!(summary.narrative.contains("[信息]"));
        assert!(summary.narrative.contains("天气晴朗"));
    }
}
