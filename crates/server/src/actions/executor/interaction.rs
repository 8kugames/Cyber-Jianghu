// ============================================================================
// 交互动作执行器
// ============================================================================
//
// 实现Agent间交互动作：give, steal, trade
// ============================================================================

use super::super::{ActionExecutionResult, StateChange};
use super::super::{GiveData, StealData, TradeData};
use crate::game_data::{ActionField, ActionRegistry};
use crate::items::get_item_definition;
use crate::models::{AgentState, Intent};
use tracing::debug;
use uuid::Uuid;

/// 交互动作执行器
pub(super) struct InteractionActionExecutor;

impl InteractionActionExecutor {
    /// 执行 give 动作
    ///
    /// 注意： MVP 阶段简化实现，实际物品转移在数据库操作中完成
    pub(super) fn execute_give(
        intent: &Intent,
        _agent_state: &mut AgentState,
    ) -> ActionExecutionResult {
        // 解析 give 动作数据
        let data: GiveData = match intent
            .action_data
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
        {
            Some(d) => d,
            None => {
                return ActionExecutionResult::failure(
                    "缺少给予数据".to_string(),
                    intent.action_type.to_string(), Some(intent.intent_id),
                );
            }
        };

        // 验证物品是否存在
        if get_item_definition(&data.item_id).is_none() {
            return ActionExecutionResult::failure(
                format!("物品不存在: {}", data.item_id),
                intent.action_type.to_string(), Some(intent.intent_id),
            );
        }

        // 验证数量有效
        if data.quantity <= 0 {
            return ActionExecutionResult::failure(
                "给予数量必须大于 0".to_string(),
                intent.action_type.to_string(), Some(intent.intent_id),
            );
        }

        // 验证目标 ID
        let target_id = match Uuid::parse_str(&data.target_agent_id) {
            Ok(id) => id,
            Err(_) => {
                return ActionExecutionResult::failure(
                    "无效的目标 ID".to_string(),
                    intent.action_type.to_string(), Some(intent.intent_id),
                );
            }
        };

        // 创建成功结果并添加状态变更
        let mut result = ActionExecutionResult::success(
            format!("给予 {} 个 {} 成功", data.quantity, data.item_id),
            intent.action_type.to_string(), Some(intent.intent_id),
        );

        // 添加物品转移变更
        result.add_change(StateChange::ItemTransferred {
            from: intent.agent_id,
            to: target_id,
            item_id: data.item_id,
            quantity: data.quantity,
        });

        result
    }

    /// 执行 steal 动作
    pub(super) fn execute_steal(
        intent: &Intent,
        _agent_state: &mut AgentState,
    ) -> ActionExecutionResult {
        // 解析 steal 动作数据
        let data: StealData = match intent
            .action_data
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
        {
            Some(d) => d,
            None => {
                return ActionExecutionResult::failure(
                    "缺少偷窃数据".to_string(),
                    intent.action_type.to_string(), Some(intent.intent_id),
                );
            }
        };

        // 验证目标 ID
        let target_id = match Uuid::parse_str(&data.target_agent_id) {
            Ok(id) => id,
            Err(_) => {
                return ActionExecutionResult::failure(
                    "无效的目标 ID".to_string(),
                    intent.action_type.to_string(), Some(intent.intent_id),
                );
            }
        };

        // 从配置读取成功率
        let success_rate = match ActionRegistry::get_f32("steal", ActionField::SuccessRate) {
            Some(rate) => rate,
            None => {
                // 配置缺失，返回失败
                return ActionExecutionResult::failure(
                    "偷窃动作配置缺失".to_string(),
                    intent.action_type.to_string(), Some(intent.intent_id),
                );
            }
        };

        let rng_roll = rand::random::<f32>();
        let success = rng_roll < success_rate;

        // 调试日志：记录 RNG 结果
        debug!(
            "[STEAL] Agent {} -> {}: RNG roll={:.4}, success_rate={:.4}, success={}",
            intent.agent_id, target_id, rng_roll, success_rate, success
        );

        if success {
            let mut result = ActionExecutionResult::success(
                format!("偷窃 {} 成功!", data.item_id),
                intent.action_type.to_string(), Some(intent.intent_id),
            );

            // 偷窃成功，物品从目标转移到自己
            // 注意：偷窃数量通常为 1，或者随机，这里简化为 1
            let quantity = 1;
            result.add_change(StateChange::ItemTransferred {
                from: target_id,
                to: intent.agent_id,
                item_id: data.item_id,
                quantity,
            });

            result
        } else {
            ActionExecutionResult::failure(
                "偷窃失败，被发现了!".to_string(),
                intent.action_type.to_string(), Some(intent.intent_id),
            )
        }
    }

    /// 执行 trade 动作
    ///
    /// 交易，带价格协商的物品转移
    /// 使用 TradeExecuted 变体进行原子处理
    pub(super) fn execute_trade(
        intent: &Intent,
        action_data: Option<serde_json::Value>,
    ) -> ActionExecutionResult {
        let data: TradeData = match action_data.and_then(|v| serde_json::from_value(v).ok()) {
            Some(d) => d,
            None => {
                return ActionExecutionResult::failure(
                    "缺少交易数据".to_string(),
                    intent.action_type.to_string(), Some(intent.intent_id),
                );
            }
        };

        // 解析目标 ID
        let target_id = match Uuid::parse_str(&data.target_agent_id) {
            Ok(id) => id,
            Err(_) => {
                return ActionExecutionResult::failure(
                    "无效的目标 ID".to_string(),
                    intent.action_type.to_string(), Some(intent.intent_id),
                );
            }
        };

        // 验证物品是否存在
        if get_item_definition(&data.item_id).is_none() {
            return ActionExecutionResult::failure(
                format!("物品不存在: {}", data.item_id),
                intent.action_type.to_string(), Some(intent.intent_id),
            );
        }

        // 验证价格有效
        if data.price < 0 {
            return ActionExecutionResult::failure(
                "交易价格不能为负数".to_string(),
                intent.action_type.to_string(), Some(intent.intent_id),
            );
        }

        let mut result = ActionExecutionResult::success(
            format!("准备交易：{} 以 {} 两银子", data.item_id, data.price),
            intent.action_type.to_string(), Some(intent.intent_id),
        );

        // 使用 TradeExecuted 进行原子交易（物品和银两在一个事务中处理）
        result.add_change(StateChange::TradeExecuted {
            initiator: intent.agent_id,
            target: target_id,
            item_id: data.item_id.clone(),
            item_quantity: 1, // 假设交易数量为 1
            price: data.price,
        });

        result
    }
}
