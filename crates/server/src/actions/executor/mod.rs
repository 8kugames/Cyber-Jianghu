// ============================================================================
// 动作执行器 - 模块入口
// ============================================================================
//
// 本模块完成动作执行的核心逻辑
// ============================================================================

mod basic;
mod combat;
mod interaction;

use sqlx::PgPool;

use super::ActionExecutionResult;
use crate::game_data::{ActionEffect, ActionRegistry, ActionRequirement};
use crate::models::{AgentState, Intent};

use basic::BasicActionExecutor;
use combat::CombatActionExecutor;
use interaction::InteractionActionExecutor;

/// 动作执行器
pub struct ActionExecutor;

impl ActionExecutor {
    pub fn new(_db_pool: PgPool) -> Self {
        Self
    }

    /// 执行动作
    ///
    /// 根据意图执行对应的动作
    /// 注意：验证逻辑已在调用前由 validator.rs 完成
    pub fn execute(&self, intent: &Intent, agent_state: &mut AgentState) -> ActionExecutionResult {
        // 0. 死亡检查：死亡的 Agent 将会被拒绝进入游戏
        if !agent_state.is_alive {
            return ActionExecutionResult::failure(
                "Agent 已死亡，无法执行此动作。请重新转生入世。".to_string(),
                intent.action_type.to_string(),
                Some(intent.intent_id),
            );
        }

        // 1. 处理通用消耗
        if let Err(e) = self.consume_requirements(intent, agent_state) {
            return ActionExecutionResult::failure(
                e,
                intent.action_type.to_string(),
                Some(intent.intent_id),
            );
        }

        // 2. 执行特定逻辑（数据驱动：字符串匹配）
        let mut result = match intent.action_type.as_str() {
            "idle" => BasicActionExecutor::execute_idle(intent),
            "speak" => BasicActionExecutor::execute_speak(intent, intent.action_data.clone()),
            "move" => BasicActionExecutor::execute_move(
                intent,
                intent.action_data.clone(),
                &agent_state.node_id.clone(),
            ),
            "give" => InteractionActionExecutor::execute_give(intent, agent_state),
            "steal" => InteractionActionExecutor::execute_steal(intent, agent_state),
            "use" => CombatActionExecutor::execute_use(intent, agent_state),
            "pickup" => BasicActionExecutor::execute_pickup(
                intent,
                intent.action_data.clone(),
                &agent_state.node_id.clone(),
            ),
            "drop" => BasicActionExecutor::execute_drop(
                intent,
                intent.action_data.clone(),
                &agent_state.node_id.clone(),
            ),
            "gather" => BasicActionExecutor::execute_gather(
                intent,
                intent.action_data.clone(),
                &agent_state.node_id.clone(),
            ),
            "craft" => BasicActionExecutor::execute_craft(intent, intent.action_data.clone()),
            "attack" => {
                CombatActionExecutor::execute_attack(intent, &intent.action_data, agent_state)
            }
            "trade" => InteractionActionExecutor::execute_trade(intent, intent.action_data.clone()),
            _ => {
                // 未知动作类型：尝试从 ActionRegistry 获取配置
                if let Some(config) = ActionRegistry::get(intent.action_type.as_str()) {
                    // 有配置但无特殊逻辑，返回通用成功
                    ActionExecutionResult::success(
                        config.description.clone(),
                        intent.action_type.to_string(),
                        Some(intent.intent_id),
                    )
                } else {
                    ActionExecutionResult::failure(
                        format!("未知的动作类型: {}", intent.action_type.as_str()),
                        intent.action_type.to_string(),
                        Some(intent.intent_id),
                    )
                }
            }
        };

        // 3. 应用通用效果
        if result.success {
            self.apply_generic_effects(intent, &mut result);
        }

        result
    }

    /// 处理通用需求消耗（数据驱动方式）
    ///
    /// 处理两种属性变化：
    /// 1. cost: 扣减属性值（正值表示扣减量）
    /// 2. recovery: 恢复属性值（正值表示恢复量）
    fn consume_requirements(
        &self,
        intent: &Intent,
        agent_state: &mut AgentState,
    ) -> Result<(), String> {
        let action_name = intent.action_type.to_string();
        if let Some(config) = ActionRegistry::get(&action_name) {
            let context = agent_state.get_formula_context();

            for req in &config.requirements {
                // 使用 requirement_type 字段分发，而非枚举模式匹配
                match req.requirement_type.as_str() {
                    ActionRequirement::REQUIREMENT_TYPE_ATTRIBUTE => {
                        // 获取属性名称
                        let attribute = req.get_attribute().unwrap_or("unknown");

                        // 处理 cost（扣减）
                        if let Some(cost) = req.get_cost() {
                            // cost 是正值，需要取负作为 delta
                            let delta = -cost;
                            if agent_state
                                .status
                                .apply_change(attribute, delta, &context)
                                .is_err()
                            {
                                return Err(format!("无法扣减属性 {} 值 {}", attribute, cost));
                            }
                        }

                        // 处理 recovery（恢复）
                        if let Some(recovery) = req.get_recovery() {
                            // recovery 是正值，直接作为 delta
                            let delta = recovery;
                            if agent_state
                                .status
                                .apply_change(attribute, delta, &context)
                                .is_err()
                            {
                                return Err(format!("无法恢复属性 {} 值 {}", attribute, recovery));
                            }
                        }
                    }
                    ActionRequirement::REQUIREMENT_TYPE_ITEM => {
                        // MVP 阶段暂不支持通用物品消耗（需要异步 DB 操作）
                    }
                    _ => {
                        // 未知类型的需求，跳过（可扩展）
                    }
                }
            }
        }
        Ok(())
    }

    /// 应用通用效果（数据驱动方式）
    fn apply_generic_effects(&self, intent: &Intent, _result: &mut ActionExecutionResult) {
        let action_name = intent.action_type.to_string();
        if let Some(config) = ActionRegistry::get(&action_name) {
            for effect in &config.effects {
                // 使用 effect_type 字段分发，而非枚举模式匹配
                match effect.effect_type.as_str() {
                    ActionEffect::EFFECT_TYPE_ATTRIBUTE_CHANGE => {
                        // 从 params 中提取参数
                        let _attribute = effect.get_str("attribute").unwrap_or("unknown");
                        let _operation = effect.get_str("operation").unwrap_or("add");
                        let _value = effect.get_i32("value"); // 或 get_f64

                        // TODO: 实现通用属性效果
                        // 当前不支持，因为需要将 value 转换为 AttributeValue
                    }
                    ActionEffect::EFFECT_TYPE_ADD_ITEM => {
                        // 暂不支持
                    }
                    _ => {
                        // 未知类型的效果，跳过（可扩展）
                    }
                }
            }
        }
    }
}
