mod basic;
mod combat;

use std::collections::HashSet;

use sqlx::PgPool;

use cyber_jianghu_protocol::AttributeValue;

use super::ActionExecutionResult;
use super::ParsedActionData;
use super::types::StateChange;
use crate::game_data::ActionRegistry;
use crate::models::{AgentState, Intent};

use basic::BasicActionExecutor;
use combat::CombatActionExecutor;

pub struct ActionExecutor;

impl ActionExecutor {
    pub fn new(_db_pool: PgPool) -> Self {
        Self
    }

    /// 执行动作
    ///
    /// 接收验证层已解析的 [`ParsedActionData`]，消除执行层的重复反序列化。
    pub fn execute(
        &self,
        intent: &Intent,
        parsed_data: &ParsedActionData,
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

        let current_loc = agent_state.node_id.clone();

        let mut result = match parsed_data {
            ParsedActionData::Yu(data) => {
                BasicActionExecutor::execute_yu(intent, data, &current_loc)
            }
            ParsedActionData::Qu(data) => {
                BasicActionExecutor::execute_qu(intent, data, &current_loc)
            }
            ParsedActionData::Yong(data) => BasicActionExecutor::execute_yong(intent, data),
            ParsedActionData::Speak(data) => BasicActionExecutor::execute_speak(intent, data),
            ParsedActionData::Move(data) => {
                BasicActionExecutor::execute_move(intent, data, &current_loc)
            }
            ParsedActionData::Observe(data) => {
                BasicActionExecutor::execute_observe(intent, data, all_states)
            }
            ParsedActionData::Attack(data) => {
                CombatActionExecutor::execute_attack(intent, data, agent_state)
            }
            ParsedActionData::Craft(data) => BasicActionExecutor::execute_craft(intent, data),
            ParsedActionData::Teach(data) => BasicActionExecutor::execute_teach(intent, data),
            ParsedActionData::None => BasicActionExecutor::execute_halt(intent),
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
                match req.requirement_type {
                    cyber_jianghu_protocol::RequirementType::Attribute => {
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
                    cyber_jianghu_protocol::RequirementType::Item => {}
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
                match effect.effect_type {
                    cyber_jianghu_protocol::EffectType::AttributeChange => {
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
                    cyber_jianghu_protocol::EffectType::AddItem => {
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
                    cyber_jianghu_protocol::EffectType::AttributeMaxChange => {
                        let attribute = effect.get_str("attribute").unwrap_or("unknown");
                        let value = effect.get_i32("value").unwrap_or(0);
                        result.add_change(StateChange::AttributeMaxChanged {
                            agent_id: intent.agent_id,
                            attribute: attribute.to_string(),
                            delta: value,
                        });
                    }
                }
            }
        }
    }
}
