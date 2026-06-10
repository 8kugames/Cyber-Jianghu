mod basic;
mod combat;

use std::collections::HashSet;

use sqlx::PgPool;

use cyber_jianghu_protocol::AttributeValue;

use super::ActionExecutionResult;
use super::types::StateChange;
use crate::game_data::{ActionEffect, ActionRegistry, ActionRequirement};
use crate::models::{AgentState, Intent};

use basic::BasicActionExecutor;
use combat::CombatActionExecutor;

pub struct ActionExecutor;

impl ActionExecutor {
    pub fn new(_db_pool: PgPool) -> Self {
        Self
    }

    /// 执行动作
    pub fn execute(
        &self,
        intent: &Intent,
        agent_state: &mut AgentState,
        all_states: &[AgentState],
    ) -> ActionExecutionResult {
        if !agent_state.is_alive {
            return ActionExecutionResult::failure(
                "Agent 已死亡，无法执行此动作。请重新转生入世。".to_string(),
                intent.action_type.to_string(),
                Some(intent.intent_id),
            );
        }

        let consumed_attrs = match self.consume_requirements(intent, agent_state) {
            Ok(attrs) => attrs,
            Err(e) => {
                return ActionExecutionResult::failure(
                    e,
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
            }
        };

        let action_data = intent.action_data.clone();
        let current_loc = agent_state.node_id.clone();

        let mut result = match intent.action_type.as_str() {
            "予" => BasicActionExecutor::execute_yu(intent, action_data, &current_loc),
            "取" => BasicActionExecutor::execute_qu(intent, action_data, &current_loc),
            "用" | "吃" | "喝" => BasicActionExecutor::execute_yong(intent, action_data),
            "说话" => BasicActionExecutor::execute_speak(intent, action_data),
            "移动" => BasicActionExecutor::execute_move(intent, action_data, &current_loc),
            "观察" => BasicActionExecutor::execute_observe(intent, action_data, all_states),
            "攻击" => CombatActionExecutor::execute_attack(intent, &action_data, agent_state),
            "休整" => BasicActionExecutor::execute_halt(intent),
            "制造" => BasicActionExecutor::execute_craft(intent, action_data),
            "教导" => BasicActionExecutor::execute_teach(intent, action_data),
            _ => {
                if let Some(config) = ActionRegistry::get(intent.action_type.as_str()) {
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

        if result.success {
            self.apply_generic_effects(intent, &mut result, &consumed_attrs);
        }

        result
    }

    fn consume_requirements(
        &self,
        intent: &Intent,
        agent_state: &mut AgentState,
    ) -> Result<HashSet<String>, String> {
        let mut consumed: HashSet<String> = HashSet::new();
        let action_name = intent.action_type.to_string();

        if action_name == "移动" {
            return Ok(consumed);
        }

        if let Some(config) = ActionRegistry::get(&action_name) {
            let context = agent_state.get_formula_context();

            for req in &config.requirements {
                match req.requirement_type.as_str() {
                    ActionRequirement::REQUIREMENT_TYPE_ATTRIBUTE => {
                        let attribute = req.get_attribute().unwrap_or("unknown");
                        if let Some(cost) = req.get_cost() {
                            let delta = -cost;
                            if agent_state
                                .status
                                .apply_change(attribute, delta, &context)
                                .is_err()
                            {
                                return Err(format!("无法扣减属性 {} 值 {}", attribute, cost));
                            }
                            consumed.insert(attribute.to_string());
                        }
                    }
                    ActionRequirement::REQUIREMENT_TYPE_ITEM => {}
                    _ => {}
                }
            }
        }
        Ok(consumed)
    }

    fn apply_generic_effects(
        &self,
        intent: &Intent,
        result: &mut ActionExecutionResult,
        skip_attrs: &HashSet<String>,
    ) {
        let action_name = intent.action_type.to_string();
        if let Some(config) = ActionRegistry::get(&action_name) {
            for effect in &config.effects {
                match effect.effect_type.as_str() {
                    ActionEffect::EFFECT_TYPE_ATTRIBUTE_CHANGE => {
                        let attribute = effect.get_str("attribute").unwrap_or("unknown");
                        if skip_attrs.contains(attribute) {
                            continue;
                        }
                        let operation = effect.get_str("operation").unwrap_or("add");
                        let value = effect.get_i32("value").unwrap_or(0);
                        let delta = match operation {
                            "sub" => -value,
                            _ => value,
                        };
                        result.add_change(StateChange::AttributeChanged {
                            agent_id: intent.agent_id,
                            attribute: attribute.to_string(),
                            delta: AttributeValue::Delta { value: delta },
                        });
                    }
                    ActionEffect::EFFECT_TYPE_ADD_ITEM => {
                        if let Some(item_id) = effect.get_str("item_id") {
                            let quantity = effect.get_i32("quantity").unwrap_or(1);
                            result.add_change(StateChange::ItemAcquired {
                                agent_id: intent.agent_id,
                                item_id: item_id.to_string(),
                                quantity,
                                source: "effect".to_string(),
                            });
                        }
                    }
                    ActionEffect::EFFECT_TYPE_ATTRIBUTE_MAX_CHANGE => {
                        let attribute = effect.get_str("attribute").unwrap_or("unknown");
                        let value = effect.get_i32("value").unwrap_or(0);
                        result.add_change(StateChange::AttributeMaxChanged {
                            agent_id: intent.agent_id,
                            attribute: attribute.to_string(),
                            delta: value,
                        });
                    }
                    _ => {}
                }
            }
        }
    }
}
