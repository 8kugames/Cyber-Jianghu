// ============================================================================
// OpenClaw Cyber-Jianghu Agent 状态配置访问器
// ============================================================================

use super::global::registry;

/// Agent 状态配置访问器
pub struct StateRegistry;

impl StateRegistry {
    /// 获取验证配置
    pub fn validation() -> crate::game_data::types::ValidationRulesData {
        registry()
            .map(|r| r.get().game_rules.data.validation.clone())
            .expect("配置未初始化，请确保 game_rules.json 已正确加载")
    }

    /// 获取属性配置
    pub fn get_attributes_config() -> Option<crate::game_data::types::UnifiedAttributesConfig> {
        registry().map(|r| r.get().attributes.clone())
    }
}
