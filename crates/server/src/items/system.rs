use crate::models::AgentState;
use cyber_jianghu_protocol::GameError;

use super::types::ItemDefinition;

// ============================================================================
// 物品效果应用
// ============================================================================

/// 应用物品效果（预留：物品使用系统）
///
/// 将物品效果应用到Agent状态
///
/// # 参数
/// - state: Agent状态（可变引用）
/// - item: 物品定义
///
/// # 返回
/// - Ok(()): 效果应用成功
/// - Err(GameError): 效果应用失败
#[allow(dead_code)]
pub fn apply_item_effect(state: &mut AgentState, item: &ItemDefinition) -> Result<(), GameError> {
    // 检查Agent是否存活
    if !state.is_alive {
        return Err(GameError::AgentDead {
            agent_id: state.agent_id,
        });
    }

    // 检查物品是否可使用
    if !item.is_usable() {
        return Err(GameError::ItemNotUsable(item.name.clone()));
    }

    // 应用效果（数据驱动方式）
    for effect in &item.effects {
        // 使用辅助方法获取整数值
        let delta_i32 = match effect.value_as_i32() {
            Some(v) => v,
            None => continue, // 不支持非数字类型
        };

        match effect.operation.as_str() {
            "add" => {
                // 使用 StatusComponent 的 apply_change 方法（带范围限制）
                let context = state.get_formula_context();
                let _ = state
                    .status
                    .apply_change(&effect.attribute, delta_i32, &context);
            }
            "set" => {
                // 直接设置值（StatusComponent::set 会应用基础范围限制）
                let _ = state.status.set(&effect.attribute, delta_i32);
            }
            "multiply" => {
                // 暂不支持乘法，忽略
            }
            _ => {
                // 未知操作，忽略（数据驱动，可扩展）
            }
        }
    }

    Ok(())
}

// pub fn use_item(...) removed
