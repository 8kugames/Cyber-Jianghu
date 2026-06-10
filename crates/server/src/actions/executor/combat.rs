use super::super::AttackData;
use super::super::{ActionExecutionResult, StateChange};
use crate::game_data::{ActionField, ActionRegistry};
use crate::models::{AgentState, Intent};

use uuid::Uuid;

pub(super) struct CombatActionExecutor;

impl CombatActionExecutor {
    /// 攻击
    pub(super) fn execute_attack(
        intent: &Intent,
        action_data: &Option<serde_json::Value>,
        agent_state: &AgentState,
    ) -> ActionExecutionResult {
        let data: AttackData = match action_data
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
        {
            Some(d) => d,
            None => {
                return ActionExecutionResult::failure(
                    "缺少攻击数据".to_string(),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
            }
        };

        let target_id = match Uuid::parse_str(&data.target_agent_id) {
            Ok(id) => id,
            Err(_) => {
                return ActionExecutionResult::failure(
                    "无效的目标 ID".to_string(),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
            }
        };

        let total_damage = if let Some(formula) =
            ActionRegistry::get_string("攻击", ActionField::DamageFormula)
        {
            let context = agent_state.get_formula_context();
            let i64_context: std::collections::HashMap<String, i64> = context
                .iter()
                .map(|(k, v)| (k.clone(), *v as i64))
                .collect();

            let weapon_bonus =
                ActionRegistry::get_i32("攻击", ActionField::WeaponBonus).unwrap_or(0);
            let weapon_multiplier =
                ActionRegistry::get_f32("攻击", ActionField::WeaponBonusMultiplier).unwrap_or(1.0);
            let mut float_extras = std::collections::HashMap::new();
            float_extras.insert("weapon_bonus".to_string(), weapon_bonus as f64);
            float_extras.insert("weapon_multiplier".to_string(), weapon_multiplier as f64);

            let engine = crate::game_data::formula_engine::FormulaEngine::new();
            match engine.evaluate_int_with_extras(&formula, &i64_context, &float_extras) {
                Ok(val) => val,
                Err(e) => {
                    return ActionExecutionResult::failure(
                        format!("伤害公式错误: {}", e),
                        intent.action_type.to_string(),
                        Some(intent.intent_id),
                    );
                }
            }
        } else {
            let base_damage = match ActionRegistry::get_i32("攻击", ActionField::BaseDamage) {
                Some(damage) => damage,
                None => {
                    return ActionExecutionResult::failure(
                        "攻击动作配置缺失".to_string(),
                        intent.action_type.to_string(),
                        Some(intent.intent_id),
                    );
                }
            };
            let weapon_bonus =
                ActionRegistry::get_i32("攻击", ActionField::WeaponBonus).unwrap_or(0);
            let weapon_multiplier =
                ActionRegistry::get_f32("攻击", ActionField::WeaponBonusMultiplier).unwrap_or(1.0);
            let weapon_damage = (weapon_bonus as f32) * weapon_multiplier;
            base_damage + weapon_damage as i32
        };

        let mut result = ActionExecutionResult::success(
            format!("攻击成功，造成 {} 点伤害", total_damage),
            intent.action_type.to_string(),
            Some(intent.intent_id),
        );

        result.add_change(StateChange::HpChanged {
            agent_id: target_id,
            delta: -total_damage,
        });

        result
    }
}
