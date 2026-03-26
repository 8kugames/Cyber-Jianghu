// ============================================================================
// 基础动作执行器
// ============================================================================
//
// 实现基础动作：idle, speak, move, pickup
// ============================================================================

use super::super::{ActionExecutionResult, StateChange};
use super::super::{CraftData, DropData, GatherData, MoveData, PickupData, SpeakData};
use crate::models::Intent;

/// 基础动作执行器
pub(super) struct BasicActionExecutor;

impl BasicActionExecutor {
    /// 执行 idle 动作
    pub(super) fn execute_idle(intent: &Intent) -> ActionExecutionResult {
        ActionExecutionResult::success(
            format!("Agent {} 休息了一会", intent.agent_id),
            intent.action_type.to_string(),
            Some(intent.intent_id),
        )
    }

    /// 执行 move 动作
    ///
    /// 现在支持实际的子场景间移动
    pub(super) fn execute_move(
        intent: &Intent,
        action_data: Option<serde_json::Value>,
        current_location: &str,
    ) -> ActionExecutionResult {
        let data: MoveData = match action_data.and_then(|v| serde_json::from_value(v).ok()) {
            Some(d) => d,
            None => {
                return ActionExecutionResult::failure(
                    "缺少移动数据".to_string(),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
            }
        };

        // 获取位置注册表
        let registry = crate::game_data::registry_or_panic();

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
                    "无法从 {} 移动到 {}（位置不相邻）",
                    current_location, data.target_location
                ),
                intent.action_type.to_string(),
                Some(intent.intent_id),
            );
        }

        let mut result = ActionExecutionResult::success(
            format!(
                "Agent {} 从 {} 移动到 {}",
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

    /// 执行 speak 动作
    pub(super) fn execute_speak(
        intent: &Intent,
        action_data: Option<serde_json::Value>,
    ) -> ActionExecutionResult {
        let data: SpeakData = match action_data.and_then(|v| serde_json::from_value(v).ok()) {
            Some(d) => d,
            None => {
                return ActionExecutionResult::failure(
                    "缺少对话数据".to_string(),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
            }
        };

        let mut result = ActionExecutionResult::success(
            format!("{} 说: {}", intent.agent_id, data.content),
            intent.action_type.to_string(),
            Some(intent.intent_id),
        );

        result.add_change(StateChange::MessageSpoken {
            agent_id: intent.agent_id,
            content: data.content,
        });

        result
    }

    /// 执行 pickup 动作
    ///
    /// 从场景中拾取地面物品
    pub(super) fn execute_pickup(
        intent: &Intent,
        action_data: Option<serde_json::Value>,
        _current_location: &str,
    ) -> ActionExecutionResult {
        let data: PickupData = match action_data.and_then(|v| serde_json::from_value(v).ok()) {
            Some(d) => d,
            None => {
                return ActionExecutionResult::failure(
                    "缺少拾取数据".to_string(),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
            }
        };

        let mut result = ActionExecutionResult::success(
            format!("尝试从场景中拾取 {} 个 {}", data.quantity, data.item_id),
            intent.action_type.to_string(),
            Some(intent.intent_id),
        );

        result.add_change(StateChange::ItemPickedUp {
            agent_id: intent.agent_id,
            item_id: data.item_id.clone(),
            quantity: data.quantity,
        });

        result
    }

    /// 执行 drop 动作
    pub(super) fn execute_drop(
        intent: &Intent,
        action_data: Option<serde_json::Value>,
        current_location: &str,
    ) -> ActionExecutionResult {
        let data: DropData = match action_data.and_then(|v| serde_json::from_value(v).ok()) {
            Some(d) => d,
            None => {
                return ActionExecutionResult::failure(
                    "缺少丢弃数据".to_string(),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
            }
        };

        let mut result = ActionExecutionResult::success(
            format!("丢弃了 {} 个 {} 到地面", data.quantity, data.item_id),
            intent.action_type.to_string(),
            Some(intent.intent_id),
        );

        result.add_change(StateChange::ItemDropped {
            from_agent: intent.agent_id,
            item_id: data.item_id.clone(),
            quantity: data.quantity,
            location: current_location.to_string(),
        });

        result
    }

    /// 执行 gather 动作
    ///
    /// 从场景中采集静态资源（校验 gatherable_items）
    pub(super) fn execute_gather(
        intent: &Intent,
        action_data: Option<serde_json::Value>,
        current_location: &str,
    ) -> ActionExecutionResult {
        let data: GatherData = match action_data.and_then(|v| serde_json::from_value(v).ok()) {
            Some(d) => d,
            None => {
                return ActionExecutionResult::failure(
                    "缺少采集数据".to_string(),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
            }
        };

        // 获取位置注册表
        let registry = crate::game_data::registry_or_panic();
        let location_registry = registry.location_registry.read().unwrap();

        // 校验当前位置是否可以采集该物品
        let can_gather = location_registry
            .get_node(current_location)
            .map(|node| node.gatherable_items.contains(&data.target_id))
            .unwrap_or(false);

        if !can_gather {
            return ActionExecutionResult::failure(
                format!("当前位置无法采集 {}", data.target_id),
                intent.action_type.to_string(),
                Some(intent.intent_id),
            );
        }

        // 季节影响产量
        let mut quantity = 1;
        if let Some(season) =
            crate::game_data::registry::TimeRegistry::get_current_season(intent.tick_id)
        {
            quantity = (quantity as f32 * season.resource_growth_rate).floor() as i32;
            if quantity < 1 {
                // 如果生长率太低（如冬季 0.2），则有概率采集失败或至少给 1 个
                // 这里 MVP 简化为，冬季即使 *0.2 变成 0，也保底给 1 个，或者随机
                // 暂时保底 1 个
                quantity = 1;
            }
        }

        let mut result = ActionExecutionResult::success(
            format!("从场景中采集了 {} 个 {}", quantity, data.target_id),
            intent.action_type.to_string(),
            Some(intent.intent_id),
        );

        result.add_change(StateChange::ItemGathered {
            agent_id: intent.agent_id,
            item_id: data.target_id.clone(),
            quantity,
        });

        result
    }

    /// 执行 craft 动作
    pub(super) fn execute_craft(
        intent: &Intent,
        action_data: Option<serde_json::Value>,
    ) -> ActionExecutionResult {
        let data: CraftData = match action_data.and_then(|v| serde_json::from_value(v).ok()) {
            Some(d) => d,
            None => {
                return ActionExecutionResult::failure(
                    "缺少制造数据".to_string(),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
            }
        };

        // 验证配方是否存在
        let recipe = match crate::game_data::registry::RecipeRegistry::get(&data.recipe_id) {
            Some(r) => r,
            None => {
                return ActionExecutionResult::failure(
                    format!("配方不存在: {}", data.recipe_id),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
            }
        };

        let mut result = ActionExecutionResult::success(
            format!("制造了 {}", recipe.name),
            intent.action_type.to_string(),
            Some(intent.intent_id),
        );

        result.add_change(StateChange::ItemCrafted {
            agent_id: intent.agent_id,
            item_id: recipe.result_item.clone(),
            quantity: recipe.result_quantity,
        });

        // 注意：目前由于基础执行器是同步的，不直接操作 DB，
        // 真正的材料扣除（如果有）应当在 state_processor 的 ItemCrafted 结算中异步完成。
        // MVP 阶段暂时不强制在 execute_craft 中检查材料充足，由 state_processor 处理，
        // 或后续重构时引入异步 DB 检查。

        result
    }
}
