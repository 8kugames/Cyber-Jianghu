// ============================================================================
// 战斗动作执行器
// ============================================================================
//
// 实现战斗相关动作：attack, use
// ============================================================================

use super::super::{ActionExecutionResult, StateChange};
use super::super::{AttackData, FleeData, UseData};
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
        if action_str == "进食" || action_str == "饮水" {
            let target_attr = if action_str == "进食" {
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
            ActionRegistry::get_string("攻击", ActionField::DamageFormula)
        {
            let context = agent_state.get_formula_context();
            let i64_context: std::collections::HashMap<String, i64> = context
                .iter()
                .map(|(k, v)| (k.clone(), *v as i64))
                .collect();

            // 武器加成作为额外变量
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

        // 注意：死亡检测由 apply_state_change 在应用 HP 变化后处理

        result
    }

    /// 执行 flee 动作
    ///
    /// 逃跑：验证相邻位置 + 公式计算成功率 + RNG 判定
    pub(super) fn execute_flee(
        intent: &Intent,
        action_data: Option<serde_json::Value>,
        current_location: &str,
        agent_state: &AgentState,
    ) -> ActionExecutionResult {
        let data: FleeData = match action_data.and_then(|v| serde_json::from_value(v).ok()) {
            Some(d) => d,
            None => {
                return ActionExecutionResult::failure(
                    "缺少逃跑目标位置".to_string(),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
            }
        };

        // 获取位置注册表
        let registry = match crate::game_data::registry_or_error() {
            Ok(r) => r,
            Err(e) => {
                return ActionExecutionResult::failure(
                    format!("注册表未初始化: {}", e),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
            }
        };

        // 验证目标位置存在
        if !registry
            .location_registry
            .read()
            .unwrap()
            .node_exists(&data.target_location)
        {
            return ActionExecutionResult::failure(
                format!("目标位置不存在: {}", data.target_location),
                intent.action_type.to_string(),
                Some(intent.intent_id),
            );
        }

        // 验证目标位置与当前位置相邻
        if !registry
            .location_registry
            .read()
            .unwrap()
            .is_connected(current_location, &data.target_location)
        {
            return ActionExecutionResult::failure(
                format!(
                    "无法从 {} 逃跑至 {}（位置不相邻）",
                    current_location, data.target_location
                ),
                intent.action_type.to_string(),
                Some(intent.intent_id),
            );
        }

        // 公式计算成功率
        let success = if let Some(formula) =
            ActionRegistry::get_string("逃跑", ActionField::FleeSuccessFormula)
        {
            let context = agent_state.get_formula_context();
            let f64_context: std::collections::HashMap<String, f64> = context
                .iter()
                .map(|(k, v)| (k.clone(), *v as f64))
                .collect();

            let engine = crate::game_data::formula_engine::FormulaEngine::new();
            match engine.evaluate(&formula, &f64_context) {
                Ok(chance) => rand::random::<f64>() < chance.clamp(0.0, 1.0),
                Err(_) => rand::random::<f64>() < 0.5,
            }
        } else {
            rand::random::<f64>() < 0.5
        };

        if !success {
            return ActionExecutionResult::failure(
                "逃跑失败，未能脱离当前位置".to_string(),
                intent.action_type.to_string(),
                Some(intent.intent_id),
            );
        }

        let mut result = ActionExecutionResult::success(
            format!(
                "Agent {} 从 {} 逃跑至 {}",
                intent.agent_id, current_location, data.target_location
            ),
            intent.action_type.to_string(),
            Some(intent.intent_id),
        );

        result.add_change(StateChange::LocationChanged {
            agent_id: intent.agent_id,
            old_location: current_location.to_string(),
            new_location: data.target_location.clone(),
        });

        result
    }
}
