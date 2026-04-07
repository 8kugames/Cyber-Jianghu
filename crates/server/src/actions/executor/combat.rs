// ============================================================================
// 战斗动作执行器
// ============================================================================
//
// 实现战斗相关动作：attack, use
// ============================================================================

use super::super::{ActionExecutionResult, StateChange};
use super::super::{AttackData, UseData};
use crate::game_data::{ActionField, ActionRegistry};
use crate::items::get_item_definition;
use crate::models::{AgentState, Intent};

use uuid::Uuid;

/// 战斗动作执行器
pub(super) struct CombatActionExecutor;

impl CombatActionExecutor {
    /// 执行 use 动作
    ///
    /// 注意：物品效果不在此处直接应用，而是作为 StateChange 返回
    /// 在 apply_state_change 中会先扣除物品，成功后再应用效果
    pub(super) fn execute_use(
        intent: &Intent,
        _agent_state: &mut AgentState,
    ) -> ActionExecutionResult {
        let data: UseData = match intent
            .action_data
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
        {
            Some(d) => d,
            None => {
                return ActionExecutionResult::failure(
                    "缺少使用数据".to_string(),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
            }
        };

        // 获取物品定义
        let item = match get_item_definition(&data.item_id) {
            Some(item) => item,
            None => {
                return ActionExecutionResult::failure(
                    format!("物品不存在: {}", data.item_id),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
            }
        };

        // 检查物品是否可使用（只有 Consumable 类型的物品可以使用）
        if !item.is_usable() {
            return ActionExecutionResult::failure(
                format!("{} 不可使用", item.name),
                intent.action_type.to_string(),
                Some(intent.intent_id),
            );
        }

        // eat/drink 语义过滤：检查物品效果是否匹配动作类型
        let action_str = intent.action_type.as_str();
        if action_str == "eat" || action_str == "drink" {
            let target_attr = if action_str == "eat" {
                "hunger"
            } else {
                "thirst"
            };
            let has_matching_effect = item
                .effects
                .iter()
                .any(|e| e.attribute == target_attr && e.operation == "add");
            if !has_matching_effect {
                return ActionExecutionResult::failure(
                    format!(
                        "{} 不能用于{}（无 {} 恢复效果）",
                        item.name, action_str, target_attr
                    ),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
            }
        }

        // 收集物品效果（在 apply_state_change 中扣除物品成功后应用）
        let effects: Vec<super::super::ItemEffect> = item
            .effects
            .iter()
            .filter_map(|effect| {
                // 支持 add, set, multiply 操作
                if effect.operation == "add"
                    || effect.operation == "set"
                    || effect.operation == "multiply"
                {
                    effect.value_as_i32().map(|v| super::super::ItemEffect {
                        attribute: effect.attribute.clone(),
                        operator: effect.operation.clone(),
                        value: v,
                    })
                } else {
                    None
                }
            })
            .collect();

        let mut result = ActionExecutionResult::success(
            format!("准备使用 {}", item.name),
            intent.action_type.to_string(),
            Some(intent.intent_id),
        );

        // 添加物品使用变更（包含效果信息，在 apply_state_change 中原子处理）
        result.add_change(StateChange::ItemUsed {
            agent_id: intent.agent_id,
            item_id: data.item_id.clone(),
            effects,
        });

        result
    }

    /// 执行 attack 动作
    ///
    /// 注意：目标验证已在 validator.rs 中完成
    /// 死亡检测将在 apply_state_change 中完成
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

        // 解析目标 ID（验证已在 validator 中完成）
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

        // 计算伤害（优先使用公式，否则使用旧版逻辑）
        let total_damage = if let Some(formula) =
            ActionRegistry::get_string("attack", ActionField::DamageFormula)
        {
            let context = agent_state.get_formula_context();
            let i64_context: std::collections::HashMap<String, i64> = context
                .iter()
                .map(|(k, v)| (k.clone(), *v as i64))
                .collect();

            // 武器加成作为额外变量
            let weapon_bonus =
                ActionRegistry::get_i32("attack", ActionField::WeaponBonus).unwrap_or(0);
            let weapon_multiplier =
                ActionRegistry::get_f32("attack", ActionField::WeaponBonusMultiplier)
                    .unwrap_or(1.0);
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
            let base_damage = match ActionRegistry::get_i32("attack", ActionField::BaseDamage) {
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
                ActionRegistry::get_i32("attack", ActionField::WeaponBonus).unwrap_or(0);
            let weapon_multiplier =
                ActionRegistry::get_f32("attack", ActionField::WeaponBonusMultiplier)
                    .unwrap_or(1.0);
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

        // 注意：死亡检测由 apply_state_change 在应用 HP 变化后处理

        result
    }
}
