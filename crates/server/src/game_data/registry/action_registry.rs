// ============================================================================
// OpenClaw Cyber-Jianghu 动作配置访问器
// ============================================================================

use super::global::registry;
use crate::game_data::types::ActionConfigEntry;

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
}

/// Action 字段枚举
#[derive(Debug, Clone, Copy)]
pub enum ActionField {
    BaseDamage,
    DamageFormula,
    WeaponBonus,
    WeaponBonusMultiplier,
    SuccessRate,
}
