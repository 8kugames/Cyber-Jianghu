// ============================================================================
// OpenClaw Cyber-Jianghu 动作配置访问器
// ============================================================================

use super::global::registry;
use super::state_registry::StateRegistry;
use crate::game_data::types::ActionConfigEntry;
use cyber_jianghu_protocol::{ActionEffectInfo, ActionRequirementInfo, AvailableAction};

/// Action 配置访问器
///
/// 提供对任意 action 配置的通用访问
/// 不预设任何 action 类型的存在
pub struct ActionRegistry;

impl ActionRegistry {
    /// 获取指定 action 的完整配置
    pub fn get(action_name: &str) -> Option<ActionConfigEntry> {
        registry().and_then(|r| r.get().actions.data.get(action_name).cloned())
    }

    /// 获取指定 action 的某个字段值（i32 类型）
    pub fn get_i32(action_name: &str, field: ActionField) -> Option<i32> {
        Self::get(action_name).and_then(|config| match field {
            ActionField::BaseDamage => config.base_damage,
            ActionField::WeaponBonus => config.weapon_bonus,
            _ => None,
        })
    }

    /// 获取指定 action 的某个字段值（f32 类型）
    pub fn get_f32(action_name: &str, field: ActionField) -> Option<f32> {
        Self::get(action_name).and_then(|config| match field {
            ActionField::SuccessRate => config.success_rate,
            ActionField::WeaponBonusMultiplier => config.weapon_bonus_multiplier,
            _ => None,
        })
    }

    /// 获取指定 action 的某个字段值（String 类型）
    pub fn get_string(action_name: &str, field: ActionField) -> Option<String> {
        Self::get(action_name).and_then(|config| match field {
            ActionField::DamageFormula => config.damage_formula,
            ActionField::FleeSuccessFormula => config.flee_success_formula,
            _ => None,
        })
    }

    /// 获取指定 action 的某个字段值（f64 类型）
    pub fn get_f64(action_name: &str, field: ActionField) -> Option<f64> {
        Self::get(action_name).and_then(|config| match field {
            ActionField::DefaultFleeSuccessRate => Some(config.default_flee_success_rate),
            _ => None,
        })
    }

    /// 获取所有已配置的 action 名称
    pub fn all_action_names() -> Vec<String> {
        registry()
            .map(|r| r.get().actions.data.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// 获取带有指定标签的 action 名称列表
    pub fn action_names_with_tag(tag: &str) -> Vec<String> {
        Self::all_action_names()
            .into_iter()
            .filter(|name| {
                Self::get(name)
                    .map(|config| config.tags.iter().any(|t| t == tag))
                    .unwrap_or(false)
            })
            .collect()
    }

    /// 构建所有可用动作的 AvailableAction 列表（数据驱动）
    pub fn build_available_actions() -> Vec<AvailableAction> {
        let display_map = Self::build_attribute_display_map();

        Self::all_action_names()
            .into_iter()
            .filter_map(|action_name| {
                let config = Self::get(&action_name)?;
                Some(AvailableAction {
                    action: action_name.clone(),
                    name: config.name.clone(),
                    description: config.description.clone(),
                    category: config.category.clone(),
                    valid_targets: None,
                    required_fields: config
                        .validation
                        .as_ref()
                        .map(|v| v.required_fields.clone())
                        .unwrap_or_default(),
                    ooc_risk: config.ooc_risk.clone(),
                    requirements: config
                        .requirements
                        .iter()
                        .map(|r| {
                            let mut params = r.params.clone();
                            Self::inject_display_name(&mut params, &display_map);
                            ActionRequirementInfo {
                                requirement_type: r.requirement_type.clone(),
                                target: r.target.clone(),
                                params,
                            }
                        })
                        .collect(),
                    effects: config
                        .effects
                        .iter()
                        .map(|e| {
                            let mut params = e.params.clone();
                            Self::inject_display_name(&mut params, &display_map);
                            ActionEffectInfo {
                                effect_type: e.effect_type.clone(),
                                target: e.target.clone(),
                                params,
                            }
                        })
                        .collect(),
                })
            })
            .collect()
    }

    /// 构建 attribute name → display_name 映射（从 attributes.yaml）
    fn build_attribute_display_map() -> std::collections::HashMap<String, String> {
        let mut map = std::collections::HashMap::new();
        if let Some(attrs) = StateRegistry::get_attributes_config() {
            for (name, def) in &attrs.data.primary.attributes {
                map.insert(name.clone(), def.display_name.clone());
            }
            for (name, def) in &attrs.data.status.attributes {
                map.insert(name.clone(), def.display_name.clone());
            }
            for (name, def) in &attrs.data.derived.attributes {
                map.insert(name.clone(), def.display_name.clone());
            }
        }
        map
    }

    /// 如果 params 含 "attribute" 字段，注入 "display_attribute" 用于渲染
    fn inject_display_name(
        params: &mut std::collections::HashMap<String, serde_json::Value>,
        display_map: &std::collections::HashMap<String, String>,
    ) {
        if let Some(attr) = params.get("attribute").and_then(|v| v.as_str())
            && let Some(display) = display_map.get(attr)
        {
            params.insert(
                "display_attribute".to_string(),
                serde_json::Value::String(display.clone()),
            );
        }
    }
}

/// Action 字段枚举
#[derive(Debug, Clone, Copy)]
pub enum ActionField {
    BaseDamage,
    DamageFormula,
    FleeSuccessFormula,
    DefaultFleeSuccessRate,
    WeaponBonus,
    WeaponBonusMultiplier,
    SuccessRate,
}
