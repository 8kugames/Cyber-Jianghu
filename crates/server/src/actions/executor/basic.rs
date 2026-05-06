// ============================================================================
// 基础动作执行器
// ============================================================================
//
// 实现基础动作：idle, speak, move, pickup
// ============================================================================

use super::super::{ActionExecutionResult, StateChange};
use super::super::{CraftData, DropData, GatherData, MoveData, PickupData, ShoutData, SpeakData};
use crate::models::Intent;

/// 基础动作执行器
pub(super) struct BasicActionExecutor;

/// 反序列化 action_data，带诊断错误信息
///
/// LLM 幻觉导致格式错误时，返回包含具体 serde 错误的 failure，
/// 让 Agent 在下一 tick 能根据错误信息自我纠正（而非"缺少xxx数据"的模糊错误）
macro_rules! deserialize_action_data {
    ($action_data:expr, $intent:expr, $type:ty, $action_type_str:expr) => {{
        match $action_data {
            Some(v) => match serde_json::from_value::<$type>(v) {
                Ok(data) => data,
                Err(e) => {
                    return ActionExecutionResult::failure(
                        format!("action_data 格式错误: {}", e),
                        $action_type_str.to_string(),
                        Some($intent.intent_id),
                    );
                }
            },
            None => {
                return ActionExecutionResult::failure(
                    format!("缺少 {} 数据", $action_type_str),
                    $action_type_str.to_string(),
                    Some($intent.intent_id),
                );
            }
        }
    }};
}

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
    /// 体力消耗 = travel_cost * 2（数据驱动）
    pub(super) fn execute_move(
        intent: &Intent,
        action_data: Option<serde_json::Value>,
        current_location: &str,
    ) -> ActionExecutionResult {
        let data: MoveData = deserialize_action_data!(action_data, intent, MoveData, "移动");

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

        let location_registry = registry.location_registry.read().unwrap();

        // 验证目标位置存在
        if !location_registry.node_exists(&data.target_location) {
            return ActionExecutionResult::failure(
                format!("目标位置不存在: {}", data.target_location),
                intent.action_type.to_string(),
                Some(intent.intent_id),
            );
        }

        // 验证目标位置与当前位置相邻
        if !location_registry.is_connected(current_location, &data.target_location) {
            return ActionExecutionResult::failure(
                format!(
                    "无法从 {} 移动到 {}（位置不相邻）",
                    current_location, data.target_location
                ),
                intent.action_type.to_string(),
                Some(intent.intent_id),
            );
        }

        // 获取 travel_cost 并计算体力消耗
        let travel_cost = location_registry
            .get_travel_cost(current_location, &data.target_location)
            .unwrap_or(1);
        let stamina_multiplier = registry
            .get()
            .game_rules
            .data
            .agent_state
            .location
            .travel_stamina_multiplier;
        let stamina_cost = travel_cost as i32 * stamina_multiplier;

        let mut result = ActionExecutionResult::success(
            format!(
                "Agent {} 从 {} 移动到 {}，消耗 {} 体力",
                intent.agent_id, current_location, data.target_location, stamina_cost
            ),
            intent.action_type.to_string(),
            Some(intent.intent_id),
        );

        // 添加体力消耗
        result.add_change(StateChange::StaminaChanged {
            agent_id: intent.agent_id,
            delta: -stamina_cost,
        });

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
        let data: SpeakData = deserialize_action_data!(action_data, intent, SpeakData, "说话");

        let mut result = ActionExecutionResult::success(
            format!("{} 说: {}", intent.agent_id, data.content),
            intent.action_type.to_string(),
            Some(intent.intent_id),
        );

        // 从 SpeakData 提取 target_agent_id
        let target_agent_id = data.target_agent_id;

        result.add_change(StateChange::MessageSpoken {
            agent_id: intent.agent_id,
            content: data.content,
            target_agent_id,
            already_broadcast: intent.already_broadcast,
        });

        result
    }

    /// 执行 shout 动作
    ///
    /// 大喊，内容对当前位置所有 Agent 可见。复用 MessageSpoken StateChange。
    pub(super) fn execute_shout(
        intent: &Intent,
        action_data: Option<serde_json::Value>,
    ) -> ActionExecutionResult {
        let data: ShoutData = deserialize_action_data!(action_data, intent, ShoutData, "大喊");

        if data.content.trim().is_empty() {
            return ActionExecutionResult::failure(
                "喊叫内容不能为空".to_string(),
                intent.action_type.to_string(),
                Some(intent.intent_id),
            );
        }

        let mut result = ActionExecutionResult::success(
            format!("{} 大喊: {}", intent.agent_id, data.content),
            intent.action_type.to_string(),
            Some(intent.intent_id),
        );

        result.add_change(StateChange::MessageSpoken {
            agent_id: intent.agent_id,
            content: data.content,
            target_agent_id: None,
            already_broadcast: intent.already_broadcast,
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
        let data: PickupData = deserialize_action_data!(action_data, intent, PickupData, "拾取");

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
        let data: DropData = deserialize_action_data!(action_data, intent, DropData, "丢弃");

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
        // 容错映射：LLM 常误用 item_id，自动修正为 target_id
        let action_data = action_data.map(|mut v| {
            if let Some(obj) = v.as_object_mut()
                && !obj.contains_key("target_id")
                && obj.contains_key("item_id")
                && let Some(val) = obj.remove("item_id")
            {
                obj.insert("target_id".to_string(), val);
            }
            v
        });
        let data: GatherData = deserialize_action_data!(action_data, intent, GatherData, "采集");

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
        let data: CraftData = deserialize_action_data!(action_data, intent, CraftData, "制造");

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
