// ============================================================================
// 动作执行器 - 模块入口
// ============================================================================
//
// 本模块完成动作执行的核心逻辑
// ============================================================================

mod basic;
mod combat;
mod interaction;

use std::collections::HashSet;

use sqlx::PgPool;

use cyber_jianghu_protocol::AttributeValue;

use super::ActionExecutionResult;
use super::types::StateChange;
use crate::game_data::{ActionEffect, ActionRegistry, ActionRequirement};
use crate::models::{AgentState, Intent};

use super::validator::normalize_action_data;

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

        // 1. 处理通用消耗（返回已扣减的属性集合，防止下游 effects 双重扣减）
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

        // 2. 规范化 action_data（LLM 字段名容错）
        let action_data = normalize_action_data(&intent.action_data);

        // 3. 执行特定逻辑（数据驱动：字符串匹配）
        let mut result = match intent.action_type.as_str() {
            "休息" => BasicActionExecutor::execute_idle(intent),
            "说话" => BasicActionExecutor::execute_speak(intent, action_data.clone()),
            "移动" => BasicActionExecutor::execute_move(
                intent,
                action_data.clone(),
                &agent_state.node_id.clone(),
            ),
            "赠送" => InteractionActionExecutor::execute_give(intent, agent_state),
            "偷窃" => InteractionActionExecutor::execute_steal(intent, agent_state),
            "使用" | "进食" | "饮水" => CombatActionExecutor::execute_use(intent, agent_state),
            "拾取" => BasicActionExecutor::execute_pickup(
                intent,
                action_data.clone(),
                &agent_state.node_id.clone(),
            ),
            "丢弃" => BasicActionExecutor::execute_drop(
                intent,
                action_data.clone(),
                &agent_state.node_id.clone(),
            ),
            "采集" => BasicActionExecutor::execute_gather(
                intent,
                action_data.clone(),
                &agent_state.node_id.clone(),
            ),
            "制造" => BasicActionExecutor::execute_craft(intent, action_data.clone()),
            "攻击" => CombatActionExecutor::execute_attack(intent, &action_data, agent_state),
            "交易" => InteractionActionExecutor::execute_trade(intent, action_data.clone()),
            "大喊" => BasicActionExecutor::execute_shout(intent, action_data.clone()),
            "修炼" => BasicActionExecutor::execute_practice(intent, action_data.clone()),
            "逃跑" => CombatActionExecutor::execute_flee(
                intent,
                action_data.clone(),
                &agent_state.node_id.clone(),
                agent_state,
            ),
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

        // 3. 应用通用效果（跳过 consume_requirements 已处理的属性，防止双重扣减）
        if result.success {
            self.apply_generic_effects(intent, &mut result, &consumed_attrs);
        }

        result
    }

    /// 处理通用需求消耗（数据驱动方式）
    ///
    /// 仅处理 cost（扣减），recovery 已迁移至 effects 管线。
    /// 返回已扣减的属性名集合，供 apply_generic_effects 跳过。
    fn consume_requirements(
        &self,
        intent: &Intent,
        agent_state: &mut AgentState,
    ) -> Result<HashSet<String>, String> {
        let mut consumed: HashSet<String> = HashSet::new();
        let action_name = intent.action_type.to_string();
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
                    ActionRequirement::REQUIREMENT_TYPE_ITEM => {
                        // MVP 阶段暂不支持通用物品消耗（需要异步 DB 操作）
                    }
                    _ => {}
                }
            }
        }
        Ok(consumed)
    }

    /// 应用通用效果（数据驱动方式）
    ///
    /// `skip_attrs`: 已被 consume_requirements 扣减的属性集合，跳过以防止双重扣减
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

                        // 跳过已被 consume_requirements 扣减的属性
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
                            result.add_change(StateChange::ItemGathered {
                                agent_id: intent.agent_id,
                                item_id: item_id.to_string(),
                                quantity,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
