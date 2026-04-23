// ============================================================================
// 交互动作执行器
// ============================================================================
//
// 实现Agent间交互动作：give, steal
// ============================================================================

use super::super::{ActionExecutionResult, StateChange};
use super::super::{GiveData, StealData};
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
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
            }
        };

        // 验证物品是否存在
        if get_item_definition(&data.item_id).is_none() {
            return ActionExecutionResult::failure(
                format!("物品不存在: {}", data.item_id),
                intent.action_type.to_string(),
                Some(intent.intent_id),
            );
        }

        // 验证数量有效
        if data.quantity <= 0 {
            return ActionExecutionResult::failure(
                "给予数量必须大于 0".to_string(),
                intent.action_type.to_string(),
                Some(intent.intent_id),
            );
        }

        // 验证目标 ID
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

        // 创建成功结果并添加状态变更
        let mut result = ActionExecutionResult::success(
            format!("给予 {} 个 {} 成功", data.quantity, data.item_id),
            intent.action_type.to_string(),
            Some(intent.intent_id),
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
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
            }
        };

        // 验证目标 ID
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

        // 从配置读取成功率
        let success_rate = match ActionRegistry::get_f32("偷窃", ActionField::SuccessRate) {
            Some(rate) => rate,
            None => {
                // 配置缺失，返回失败
                return ActionExecutionResult::failure(
                    "偷窃动作配置缺失".to_string(),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
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
                intent.action_type.to_string(),
                Some(intent.intent_id),
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
                intent.action_type.to_string(),
                Some(intent.intent_id),
            )
        }
    }
}
