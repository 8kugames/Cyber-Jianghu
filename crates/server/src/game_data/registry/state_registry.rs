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

    /// 查询某 status 属性的 max_value。
    ///
    /// 复用真实原语 `StatusComponent::evaluate_max_value`（status_component.rs:189），
    /// 从 `get_attributes_config()` 取该属性的 `max_value_formula` 后求值，不自解 formula。
    pub fn get_attribute_max_value(attr_name: &str) -> Option<f32> {
        let cfg = Self::get_attributes_config()?;
        let attr = cfg.data.status.attributes.get(attr_name)?;
        // StatusAttributeDefinition.default_value 是 Option<f64>，原语期望 f32
        let default_max = attr.default_value.unwrap_or(0.0) as f32;
        let context = std::collections::HashMap::new();
        Some(crate::game_data::types::StatusComponent::evaluate_max_value(
            &attr.max_value_formula,
            default_max,
            &context,
        ))
    }
}
