// ============================================================================
// Prompt 缓存模块 - 人魂 Prompt 静态数据缓存
// ============================================================================
//
// 仅保留：
// - persona（统一摘要，结构化格式）
// - Action Index（name-only，进程生命周期内不变）
//
// 动态数据（WorldState/Skills）由 FocusSummary + 地魂 tool calling 按需获取。
// ============================================================================

use crate::component::persona::DynamicPersona;

/// Prompt 缓存状态
///
/// 仅保留 persona 差异化缓存和 Action Index（name-only）。
pub struct PromptCache {
    persona_desc: String,
    persona_summary: String,
    /// Action Index（name-only，详情通过地魂 get_action_detail 按需查询）
    action_descriptions: String,
}

impl PromptCache {
    /// 创建新的 PromptCache
    pub fn new(
        persona_desc: String,
        action_descriptions: String,
        _action_field_hints: String,
        persona: &DynamicPersona,
    ) -> Self {
        let persona_summary = Self::build_structured_summary(persona);
        Self {
            persona_desc,
            persona_summary,
            action_descriptions,
        }
    }

    /// 构建结构化 persona 摘要
    pub fn build_structured_summary(persona: &DynamicPersona) -> String {
        let traits: Vec<String> = persona
            .traits
            .iter()
            .map(|(name, trait_val)| {
                let normalized_value = trait_val.value as f64 / 100.0;
                format!(
                    "{}{}",
                    name,
                    if normalized_value > 0.7 {
                        "（强烈倾向）"
                    } else if normalized_value > 0.5 {
                        "（倾向）"
                    } else if normalized_value < 0.3 {
                        "（回避）"
                    } else {
                        ""
                    }
                )
            })
            .collect();

        let traits_str = if traits.is_empty() {
            "待探索".to_string()
        } else {
            traits.join("、")
        };

        let state_str = if persona.current_state.current_emotion != "平静" {
            format!("（当前心境：{}）", persona.current_state.current_emotion)
        } else {
            String::new()
        };

        format!(
            "你是 {}，核心特质：{}{}",
            persona.name, traits_str, state_str
        )
    }

    /// 获取 persona（统一摘要格式）
    pub fn get_persona_simple(&self) -> &str {
        &self.persona_summary
    }

    /// 获取 persona（保留兼容接口）
    pub fn get_persona(&self) -> &str {
        self.get_persona_simple()
    }

    /// 失效 persona 缓存（rebirth 后调用）
    pub fn invalidate_persona(&mut self, persona_desc: String, persona: &DynamicPersona) {
        self.persona_desc = persona_desc;
        self.persona_summary = Self::build_structured_summary(persona);
    }

    /// 更新动作描述（game_rules_update 后调用）
    pub fn update_action_descriptions(
        &mut self,
        action_descriptions: String,
        _action_field_hints: String,
    ) {
        self.action_descriptions = action_descriptions;
    }

    /// 获取 Action Index（name-only）
    pub fn get_action_descriptions(&self) -> &str {
        &self.action_descriptions
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_persona() -> DynamicPersona {
        let agent_id = uuid::Uuid::new_v4();
        DynamicPersona::new(agent_id, "张三", "你是一名行侠仗义的侠客。")
    }

    #[test]
    fn test_structured_summary() {
        let persona = create_test_persona();
        let cache = PromptCache::new(
            "你是一名行侠仗义的侠客。".to_string(),
            "- idle: 休整".to_string(),
            "- idle: (action_data: null)".to_string(),
            &persona,
        );

        let summary = cache.get_persona_simple();
        assert!(summary.contains("张三"));
        assert!(summary.contains("核心特质"));
    }

    #[test]
    fn test_always_returns_summary() {
        let persona = create_test_persona();
        let cache = PromptCache::new(
            "你是一名行侠仗义的侠客。".to_string(),
            "- idle: 休整".to_string(),
            "- idle: (action_data: null)".to_string(),
            &persona,
        );

        let first = cache.get_persona_simple();
        let second = cache.get_persona_simple();
        assert!(first.contains("张三"));
        assert!(second.contains("张三"));
        assert_eq!(first, second);
    }

    #[test]
    fn test_invalidate_updates_summary() {
        let persona = create_test_persona();
        let mut cache = PromptCache::new(
            "旧描述".to_string(),
            "- idle: 休整".to_string(),
            "- idle: (action_data: null)".to_string(),
            &persona,
        );

        let before = cache.get_persona_simple().to_string();
        assert!(before.contains("张三"));

        cache.invalidate_persona("新描述".to_string(), &persona);

        let after = cache.get_persona_simple();
        assert!(after.contains("张三"));
        assert_eq!(before, after);
    }
}
