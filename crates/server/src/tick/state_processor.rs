// ============================================================================
// OpenClaw Cyber-Jianghu MVP - State Processor Module
// ============================================================================
//
// 本模块负责处理Agent状态的变更和意图结算
//
// 功能：
// - 结算意图（处理动作）
// - 应用状态变更（属性、HP、物品等）
// - 生成WorldEvent
// ============================================================================

use anyhow::Result;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::actions::{validate_action, ActionExecutor, StateChange};
use crate::db::DbPool;
use crate::models::{ActionResult, ActionType, AgentAction, AgentState, Intent, WorldEvent};

/// 结算意图（处理动作）
///
/// 遍历所有意图，验证并执行动作
///
/// 返回值：(处理后的Agent状态, 执行的动作数, 事件列表, 动作日志)
pub async fn resolve_intents(
    db_pool: &DbPool,
    tick_id: i64,
    mut agent_states: Vec<AgentState>,
    intents: &[Intent],
) -> Result<(
    Vec<AgentState>,
    usize,
    Vec<(Uuid, WorldEvent)>,
    Vec<AgentAction>,
)> {
    let mut actions_executed = 0;
    let executor = ActionExecutor::new(db_pool.clone());
    let mut events = Vec::new();
    let mut action_logs = Vec::new();

    // 遍历所有意图并执行
    for intent in intents {
        // 校验 tick_id 一致性
        if intent.tick_id != tick_id {
            warn!(
                "意图 tick_id 不匹配: agent={}, intent_tick={}, current_tick={}, 跳过执行",
                intent.agent_id, intent.tick_id, tick_id
            );
            continue;
        }

        // 更新 Agent 在线时间
        if let Err(e) = crate::db::update_agent_online(db_pool, intent.agent_id).await {
            warn!("更新 Agent {} 在线时间失败: {}", intent.agent_id, e);
        }

        // 查找对应的 Agent 索引
        let agent_idx = match agent_states
            .iter()
            .position(|s| s.agent_id == intent.agent_id)
        {
            Some(idx) => idx,
            None => {
                warn!("意图来自未知 Agent: {}", intent.agent_id);
                continue;
            }
        };

        // 验证动作（使用不可变引用）
        if let Err(e) = validate_action(intent, &agent_states[agent_idx], &agent_states) {
            debug!("动作验证失败: agent={}, error={}", intent.agent_id, e);
            continue;
        }

        // 执行动作（验证已完成，直接执行）
        let result = executor.execute(intent, &mut agent_states[agent_idx]);

        // 记录动作日志（初始结果基于 executor 返回值）
        let action_type = ActionType::new(&result.action_type);

        let mut action_log = AgentAction {
            id: 0, // 数据库自动生成
            tick_id,
            agent_id: intent.agent_id,
            action_type,
            action_data: intent.action_data.clone(),
            result: if result.success {
                ActionResult::Success
            } else {
                ActionResult::Failed
            },
            created_at: chrono::Utc::now(),
        };

        if result.success {
            debug!(
                "动作执行成功: agent={}, action={}",
                intent.agent_id, result.action_type
            );

            // 应用状态变更，并跟踪实际应用结果
            let mut all_changes_applied = true;
            for change in &result.state_changes {
                let applied = apply_state_change(db_pool, tick_id, change, &mut agent_states, &mut events).await;
                if !applied {
                    all_changes_applied = false;
                }
            }

            // 如果所有状态变更都成功应用，则动作真正成功
            if all_changes_applied {
                actions_executed += 1;
            } else {
                // 部分状态变更失败，更新日志为失败
                warn!(
                    "动作状态变更应用失败: agent={}, action={}",
                    intent.agent_id, result.action_type
                );
                action_log.result = ActionResult::Failed;
            }
        } else {
            warn!(
                "动作执行失败: agent={}, error={}",
                intent.agent_id, result.message
            );
        }

        action_logs.push(action_log);
    }

    Ok((agent_states, actions_executed, events, action_logs))
}

/// 应用状态变更
///
/// 返回值：true 表示状态变更成功应用，false 表示应用失败
async fn apply_state_change(
    db_pool: &DbPool,
    tick_id: i64,
    change: &StateChange,
    agent_states: &mut [AgentState],
    events: &mut Vec<(Uuid, WorldEvent)>,
) -> bool {
    match change {
        StateChange::AttributeChanged {
            agent_id,
            attribute,
            delta,
        } => {
            if let Some(state) = agent_states.iter_mut().find(|s| s.agent_id == *agent_id) {
                // 使用 StatusComponent 应用变更（带范围限制）
                let delta_i32 = delta.get();
                let context = state.get_formula_context();
                let was_alive = state.is_alive;
                if let Ok(_new_val) = state.status.apply_change(attribute, delta_i32, &context) {
                        // 检查死亡条件
                    if state.status.check_death_condition(attribute) {
                        state.is_alive = false;
                        let _ = state.status.set("hp", 0); // 确保 HP 归零
                        warn!("Agent {} 因 {} 归零而死亡", agent_id, attribute);

                        if was_alive && !state.inventory_cleared_this_tick {
                            state.inventory_cleared_this_tick = true;
                            let location = state.node_id.clone();
                            match crate::inventory::InventoryManager::clear_inventory(db_pool, *agent_id).await {
                                Ok(items) => {
                                    // 死亡掉落物品到地面
                                    for item in items {
                                        if let Err(e) = crate::db::add_ground_item(db_pool, &location, &item.item_id, item.quantity, Some(*agent_id)).await {
                                            warn!("死亡掉落物品添加到地面失败: {}", e);
                                        }
                                    }
                                }
                                Err(e) => warn!("清空死亡Agent {} 背包失败: {}", agent_id, e),
                            }
                        }

                        // 记录死亡事件
                        let event = WorldEvent {
                            event_type: "action_result".to_string(),
                            tick_id,
                            description: format!("你因 {} 耗尽而死亡", attribute),
                            metadata: serde_json::json!({
                                "cause": "death",
                                "reason": attribute,
                            }),
                        };
                        events.push((*agent_id, event));
                    }
                }
            }
            true // 属性变更始终成功（内存操作）
        }
        StateChange::HpChanged { agent_id, delta } => {
            if let Some(state) = agent_states.iter_mut().find(|s| s.agent_id == *agent_id) {
                let context = state.get_formula_context();
                let was_alive = state.is_alive;
                if let Ok(new_hp) = state.status.apply_change("hp", *delta, &context) {
                    if new_hp == 0 {
                        state.is_alive = false;
                    }
                }

                // 记录 HP 变化事件（伤害或治疗）
                if *delta < 0 {
                    // 受到伤害
                    let event = WorldEvent {
                        event_type: "action_result".to_string(),
                        tick_id,
                        description: format!("你受到了 {} 点伤害", delta.abs()),
                        metadata: serde_json::json!({
                            "cause": "damage",
                            "delta": delta,
                        }),
                    };
                    events.push((*agent_id, event));
                } else if *delta > 0 {
                    // 获得治疗
                    let event = WorldEvent {
                        event_type: "action_result".to_string(),
                        tick_id,
                        description: format!("你恢复了 {} 点 HP", delta),
                        metadata: serde_json::json!({
                            "cause": "healing",
                            "delta": delta,
                        }),
                    };
                    events.push((*agent_id, event));
                }

                // 检查是否因 HP 归零死亡
                if was_alive && !state.is_alive && !state.inventory_cleared_this_tick {
                    state.inventory_cleared_this_tick = true;
                    let location = state.node_id.clone();
                    match crate::inventory::InventoryManager::clear_inventory(db_pool, *agent_id).await {
                        Ok(items) => {
                            for item in items {
                                if let Err(e) = crate::db::add_ground_item(db_pool, &location, &item.item_id, item.quantity, Some(*agent_id)).await {
                                    warn!("死亡掉落物品添加到地面失败: {}", e);
                                }
                            }
                        }
                        Err(e) => warn!("清空死亡Agent {} 背包失败: {}", agent_id, e),
                    }
                    let death_event = WorldEvent {
                        event_type: "action_result".to_string(),
                        tick_id,
                        description: "你因重伤而死亡".to_string(),
                        metadata: serde_json::json!({
                            "cause": "death",
                            "reason": "hp_zero",
                        }),
                    };
                    events.push((*agent_id, death_event));
                }
            }
            true // HP 变更始终成功（内存操作）
        }
        StateChange::HungerChanged { agent_id, delta } => {
            if let Some(state) = agent_states.iter_mut().find(|s| s.agent_id == *agent_id) {
                let context = state.get_formula_context();
                let _ = state.status.apply_change("hunger", *delta, &context);
                // 饥饿值变化通常不产生事件
            }
            true // 饥饿值变更始终成功（内存操作）
        }
        StateChange::ThirstChanged { agent_id, delta } => {
            if let Some(state) = agent_states.iter_mut().find(|s| s.agent_id == *agent_id) {
                let context = state.get_formula_context();
                let _ = state.status.apply_change("thirst", *delta, &context);
                // 口渴值变化通常不产生事件
            }
            true // 口渴值变更始终成功（内存操作）
        }
        StateChange::StaminaChanged { agent_id, delta } => {
            if let Some(state) = agent_states.iter_mut().find(|s| s.agent_id == *agent_id) {
                let context = state.get_formula_context();
                let _ = state.status.apply_change("stamina", *delta, &context);
                // 体力值变化通常不产生事件
            }
            true // 体力值变更始终成功（内存操作）
        }
        StateChange::ItemTransferred {
            from,
            to,
            item_id,
            quantity,
        } => {
            // 物品转移由 InventoryManager 处理
            let result = crate::inventory::InventoryManager::transfer_item(
                db_pool, *from, *to, item_id, *quantity,
            )
            .await;

            if let Err(e) = result {
                warn!("物品转移失败: {}", e);
                // 记录失败事件
                let event = WorldEvent {
                    event_type: "action_result".to_string(),
                    tick_id,
                    description: format!("转移失败，你没有足够的 {}", item_id),
                    metadata: serde_json::json!({
                        "action": "transfer_failed",
                        "item_id": item_id,
                        "reason": e.to_string(),
                    }),
                };
                events.push((*from, event));
                false // 转移失败
            } else {
                // 为发送方记录事件
                let event = WorldEvent {
                    event_type: "action_result".to_string(),
                    tick_id,
                    description: format!("你给 {} 转移了 {} 个 {}", to, quantity, item_id),
                    metadata: serde_json::json!({
                        "action": "give",
                        "target": to.to_string(),
                        "item_id": item_id,
                        "quantity": quantity,
                    }),
                };
                events.push((*from, event));

                // 为接收方记录事件
                let event = WorldEvent {
                    event_type: "action_result".to_string(),
                    tick_id,
                    description: format!("{} 给你转移了 {} 个 {}", from, quantity, item_id),
                    metadata: serde_json::json!({
                        "action": "receive",
                        "from": from.to_string(),
                        "item_id": item_id,
                        "quantity": quantity,
                    }),
                };
                events.push((*to, event));
                true // 转移成功
            }
        }
        StateChange::TradeExecuted {
            initiator,
            target,
            item_id,
            item_quantity,
            price,
        } => {
            // 原子交易：使用事务同时转移物品和银两
            // 任何一步失败都会回滚整个交易
            let result = async {
                // 开启事务
                let mut tx = match db_pool.begin().await {
                    Ok(tx) => tx,
                    Err(e) => {
                        warn!("交易失败：无法开启事务: {}", e);
                        return Err(format!("交易失败：数据库错误"));
                    }
                };

                // 1. 检查并扣除发起者的物品
                let available: Option<i32> = sqlx::query_scalar(
                    "SELECT quantity FROM agent_inventory WHERE agent_id = $1 AND item_id = $2 FOR UPDATE",
                )
                .bind(*initiator)
                .bind(item_id)
                .fetch_optional(&mut *tx)
                .await
                .ok()
                .flatten();

                let available = available.unwrap_or(0);
                if available < *item_quantity {
                    return Err(format!("物品数量不足: 需要 {}, 拥有 {}", item_quantity, available));
                }

                // 扣除物品
                if available == *item_quantity {
                    sqlx::query("DELETE FROM agent_inventory WHERE agent_id = $1 AND item_id = $2")
                        .bind(*initiator)
                        .bind(item_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| format!("扣除物品失败: {}", e))?;
                } else {
                    sqlx::query("UPDATE agent_inventory SET quantity = $1 WHERE agent_id = $2 AND item_id = $3")
                        .bind(available - item_quantity)
                        .bind(*initiator)
                        .bind(item_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| format!("更新物品数量失败: {}", e))?;
                }

                // 2. 检查并扣除目标的银两
                let silver_available: Option<i32> = sqlx::query_scalar(
                    "SELECT quantity FROM agent_inventory WHERE agent_id = $1 AND item_id = 'silver' FOR UPDATE",
                )
                .bind(*target)
                .fetch_optional(&mut *tx)
                .await
                .ok()
                .flatten();

                let silver_available = silver_available.unwrap_or(0);
                if silver_available < *price {
                    return Err(format!("银两不足: 需要 {}, 拥有 {}", price, silver_available));
                }

                if *price > 0 {
                    if silver_available == *price {
                        sqlx::query("DELETE FROM agent_inventory WHERE agent_id = $1 AND item_id = 'silver'")
                            .bind(*target)
                            .execute(&mut *tx)
                            .await
                            .map_err(|e| format!("扣除银两失败: {}", e))?;
                    } else {
                        sqlx::query("UPDATE agent_inventory SET quantity = $1 WHERE agent_id = $2 AND item_id = 'silver'")
                            .bind(silver_available - price)
                            .bind(*target)
                            .execute(&mut *tx)
                            .await
                            .map_err(|e| format!("更新银两数量失败: {}", e))?;
                    }
                }

                // 3. 给目标添加物品（使用 FOR UPDATE 锁定，防止并发交易）
                let target_has_item: Option<i32> = sqlx::query_scalar(
                    "SELECT quantity FROM agent_inventory WHERE agent_id = $1 AND item_id = $2 FOR UPDATE",
                )
                .bind(*target)
                .bind(item_id)
                .fetch_optional(&mut *tx)
                .await
                .ok()
                .flatten();

                if let Some(qty) = target_has_item {
                    sqlx::query("UPDATE agent_inventory SET quantity = $1 WHERE agent_id = $2 AND item_id = $3")
                        .bind(qty + item_quantity)
                        .bind(*target)
                        .bind(item_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| format!("给目标添加物品失败: {}", e))?;
                } else {
                    // 检查目标背包格子
                    let slot_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_inventory WHERE agent_id = $1")
                        .bind(*target)
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(|e| format!("检查目标背包失败: {}", e))?;

                    if slot_count >= crate::inventory::get_max_slots() as i64 {
                        return Err("目标背包已满".to_string());
                    }

                    sqlx::query("INSERT INTO agent_inventory (agent_id, item_id, quantity, is_equipped) VALUES ($1, $2, $3, false)")
                        .bind(*target)
                        .bind(item_id)
                        .bind(item_quantity)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| format!("给目标插入物品失败: {}", e))?;
                }

                // 4. 给发起者添加银两
                if *price > 0 {
                    let initiator_has_silver: Option<i32> = sqlx::query_scalar(
                        "SELECT quantity FROM agent_inventory WHERE agent_id = $1 AND item_id = 'silver'",
                    )
                    .bind(*initiator)
                    .fetch_optional(&mut *tx)
                    .await
                    .ok()
                    .flatten();

                    if let Some(qty) = initiator_has_silver {
                        sqlx::query("UPDATE agent_inventory SET quantity = $1 WHERE agent_id = $2 AND item_id = 'silver'")
                            .bind(qty + price)
                            .bind(*initiator)
                            .execute(&mut *tx)
                            .await
                            .map_err(|e| format!("给发起者添加银两失败: {}", e))?;
                    } else {
                        sqlx::query("INSERT INTO agent_inventory (agent_id, item_id, quantity, is_equipped) VALUES ($1, $2, $3, false)")
                            .bind(*initiator)
                            .bind("silver")
                            .bind(price)
                            .execute(&mut *tx)
                            .await
                            .map_err(|e| format!("给发起者插入银两失败: {}", e))?;
                    }
                }

                // 提交事务
                tx.commit().await.map_err(|e| format!("提交事务失败: {}", e))?;

                Ok(())
            }.await;

            match result {
                Ok(()) => {
                    // 交易成功，记录事件
                    let initiator_event = WorldEvent {
                        event_type: "action_result".to_string(),
                        tick_id,
                        description: format!("你以 {} 两银子出售了 {} 个 {}", price, item_quantity, item_id),
                        metadata: serde_json::json!({
                            "action": "trade",
                            "target": target.to_string(),
                            "item_id": item_id,
                            "quantity": item_quantity,
                            "price": price,
                        }),
                    };
                    events.push((*initiator, initiator_event));

                    let target_event = WorldEvent {
                        event_type: "action_result".to_string(),
                        tick_id,
                        description: format!("你以 {} 两银子购买了 {} 个 {}", price, item_quantity, item_id),
                        metadata: serde_json::json!({
                            "action": "trade",
                            "from": initiator.to_string(),
                            "item_id": item_id,
                            "quantity": item_quantity,
                            "price": price,
                        }),
                    };
                    events.push((*target, target_event));
                    true // 交易成功
                }
                Err(e) => {
                    warn!("交易失败: {}", e);
                    let event = WorldEvent {
                        event_type: "action_result".to_string(),
                        tick_id,
                        description: format!("交易失败: {}", e),
                        metadata: serde_json::json!({
                            "action": "trade_failed",
                            "reason": e,
                        }),
                    };
                    events.push((*initiator, event));
                    false // 交易失败
                }
            }
        }
        StateChange::ItemUsed { agent_id, item_id, effects } => {
            // 物品使用：先从背包移除，成功后再应用效果
            let remove_result = crate::inventory::InventoryManager::remove_item(db_pool, *agent_id, item_id, 1)
                .await;

            if let Err(e) = remove_result {
                warn!("移除物品失败（物品不存在或数量不足）: agent={}, item={}, error={}", agent_id, item_id, e);
                // 物品不存在或数量不足，不应用效果
                let event = WorldEvent {
                    event_type: "action_result".to_string(),
                    tick_id,
                    description: format!("使用失败，你没有 {}", item_id),
                    metadata: serde_json::json!({
                        "action": "use_failed",
                        "item_id": item_id,
                        "reason": "item_not_found",
                    }),
                };
                events.push((*agent_id, event));
                false // 物品使用失败
            } else {
                // 物品扣除成功，应用效果
                if let Some(state) = agent_states.iter_mut().find(|s| s.agent_id == *agent_id) {
                    let context = state.get_formula_context();
                    for effect in effects {
                        // 根据操作符应用效果
                        let value_to_apply = match effect.operator.as_str() {
                            "set" => {
                                // set 操作：先获取当前值，然后计算差值
                                if let Ok(current_value) = state.status.collection.attributes.get(&effect.attribute)
                                    .map(|attr| attr.value.get())
                                    .ok_or("attribute_not_found")
                                {
                                    effect.value - current_value
                                } else {
                                    effect.value // 如果无法获取当前值，直接使用效果值
                                }
                            }
                            "multiply" => {
                                // multiply 操作：先获取当前值，然后计算差值
                                if let Ok(current_value) = state.status.collection.attributes.get(&effect.attribute)
                                    .map(|attr| attr.value.get())
                                    .ok_or("attribute_not_found")
                                {
                                    (current_value * effect.value) - current_value
                                } else {
                                    effect.value // 如果无法获取当前值，直接使用效果值
                                }
                            }
                            _ => {
                                // add 操作（默认）
                                effect.value
                            }
                        };

                        // 应用效果
                        let _ = state.status.apply_change(&effect.attribute, value_to_apply, &context);
                    }
                }

                // 记录物品使用事件
                let event = WorldEvent {
                    event_type: "action_result".to_string(),
                    tick_id,
                    description: format!("你使用了 {}", item_id),
                    metadata: serde_json::json!({
                        "action": "use",
                        "item_id": item_id,
                    }),
                };
                events.push((*agent_id, event));
                true // 物品使用成功
            }
        }
        StateChange::ItemPickedUp {
            agent_id,
            item_id,
            quantity,
        } => {
            // 首先尝试从地面移除物品
            if let Some(state) = agent_states.iter().find(|s| s.agent_id == *agent_id) {
                let node_id = state.node_id.clone();
                let result = match crate::db::remove_ground_item(db_pool, &node_id, item_id, *quantity).await {
                    Ok(true) => {
                        // 地面有物品且移除成功，添加到背包
                        if let Err(e) = crate::inventory::InventoryManager::add_item(
                            db_pool, *agent_id, item_id, *quantity,
                        )
                        .await
                        {
                            warn!("拾取物品添加到背包失败: {}，尝试放回地面", e);
                            // 补偿机制，重新放回地面
                            if let Err(rollback_e) = crate::db::add_ground_item(db_pool, &node_id, item_id, *quantity, None).await {
                                warn!("严重错误: 物品放回地面失败，物品丢失: {}", rollback_e);
                            }

                            let event = WorldEvent {
                                event_type: "action_result".to_string(),
                                tick_id,
                                description: format!("拾取失败，背包已满或发生错误"),
                                metadata: serde_json::json!({
                                    "action": "pickup_failed",
                                    "item_id": item_id,
                                    "reason": e.to_string(),
                                }),
                            };
                            events.push((*agent_id, event));
                            false // 拾取失败
                        } else {
                            let event = WorldEvent {
                                event_type: "action_result".to_string(),
                                tick_id,
                                description: format!("你拾取了 {} 个 {}", quantity, item_id),
                                metadata: serde_json::json!({
                                    "action": "pickup",
                                    "item_id": item_id,
                                    "quantity": quantity,
                                }),
                            };
                            events.push((*agent_id, event));
                            true // 拾取成功
                        }
                    }
                    Ok(false) | Err(_) => {
                        warn!("地面没有足够的 {} 供拾取", item_id);
                        let event = WorldEvent {
                            event_type: "action_result".to_string(),
                            tick_id,
                            description: format!("拾取失败，地面没有 {}", item_id),
                            metadata: serde_json::json!({
                                "action": "pickup_failed",
                                "item_id": item_id,
                            }),
                        };
                        events.push((*agent_id, event));
                        false // 地面没有物品
                    }
                };
                result
            } else {
                false // Agent 状态未找到
            }
        }
        StateChange::ItemGathered {
            agent_id,
            item_id,
            quantity,
        } => {
            // 物品采集：添加到背包
            let result = if let Err(e) = crate::inventory::InventoryManager::add_item(
                db_pool, *agent_id, item_id, *quantity,
            )
            .await
            {
                warn!("采集物品失败: {}", e);
                false // 采集失败
            } else {
                let event = WorldEvent {
                    event_type: "action_result".to_string(),
                    tick_id,
                    description: format!("你采集了 {} 个 {}", quantity, item_id),
                    metadata: serde_json::json!({
                        "action": "gather",
                        "item_id": item_id,
                        "quantity": quantity,
                    }),
                };
                events.push((*agent_id, event));
                true // 采集成功
            };
            result
        }
        StateChange::ItemCrafted {
            agent_id,
            item_id,
            quantity,
        } => {
            // 查找对应的配方
            let recipe_to_craft = {
                let cache = crate::game_data::registry_or_panic().get();
                cache.recipes.data.values()
                    .find(|r| r.result_item == *item_id)
                    .cloned()
            };

            if let Some(recipe) = recipe_to_craft {
                // 使用事务进行原子操作
                let craft_result = async {
                    // 开启事务
                    let mut tx = match db_pool.begin().await {
                        Ok(tx) => tx,
                        Err(e) => {
                            return Err(format!("无法开启事务: {}", e));
                        }
                    };

                    // 1. 检查并锁定所有材料
                    for mat in &recipe.materials {
                        let count: i32 = sqlx::query_scalar(
                            "SELECT quantity FROM agent_inventory WHERE agent_id = $1 AND item_id = $2 FOR UPDATE",
                        )
                        .bind(*agent_id)
                        .bind(&mat.item_id)
                        .fetch_optional(&mut *tx)
                        .await
                        .map_err(|e| format!("查询材料失败: {}", e))?
                        .unwrap_or(0);

                        if count < mat.quantity {
                            return Err(format!("材料不足: {} (需要 {}, 拥有 {})", mat.item_id, mat.quantity, count));
                        }
                    }

                    // 2. 扣除所有材料
                    for mat in &recipe.materials {
                        let current: i32 = sqlx::query_scalar(
                            "SELECT quantity FROM agent_inventory WHERE agent_id = $1 AND item_id = $2",
                        )
                        .bind(*agent_id)
                        .bind(&mat.item_id)
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(|e| format!("获取材料数量失败: {}", e))?;

                        let new_qty = current - mat.quantity;
                        if new_qty <= 0 {
                            sqlx::query("DELETE FROM agent_inventory WHERE agent_id = $1 AND item_id = $2")
                                .bind(*agent_id)
                                .bind(&mat.item_id)
                                .execute(&mut *tx)
                                .await
                                .map_err(|e| format!("删除材料失败: {}", e))?;
                        } else {
                            sqlx::query("UPDATE agent_inventory SET quantity = $1 WHERE agent_id = $2 AND item_id = $3")
                                .bind(new_qty)
                                .bind(*agent_id)
                                .bind(&mat.item_id)
                                .execute(&mut *tx)
                                .await
                                .map_err(|e| format!("更新材料数量失败: {}", e))?;
                        }
                    }

                    // 3. 添加成品
                    let existing: Option<i32> = sqlx::query_scalar(
                        "SELECT quantity FROM agent_inventory WHERE agent_id = $1 AND item_id = $2",
                    )
                    .bind(*agent_id)
                    .bind(item_id)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(|e| format!("查询成品失败: {}", e))?;

                    if let Some(qty) = existing {
                        sqlx::query("UPDATE agent_inventory SET quantity = $1 WHERE agent_id = $2 AND item_id = $3")
                            .bind(qty + quantity)
                            .bind(*agent_id)
                            .bind(item_id)
                            .execute(&mut *tx)
                            .await
                            .map_err(|e| format!("更新成品数量失败: {}", e))?;
                    } else {
                        // 检查背包格子
                        let slot_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_inventory WHERE agent_id = $1")
                            .bind(*agent_id)
                            .fetch_one(&mut *tx)
                            .await
                            .map_err(|e| format!("检查背包格子失败: {}", e))?;

                        if slot_count >= crate::inventory::get_max_slots() as i64 {
                            return Err("背包已满".to_string());
                        }

                        sqlx::query("INSERT INTO agent_inventory (agent_id, item_id, quantity, is_equipped) VALUES ($1, $2, $3, false)")
                            .bind(*agent_id)
                            .bind(item_id)
                            .bind(*quantity)
                            .execute(&mut *tx)
                            .await
                            .map_err(|e| format!("插入成品失败: {}", e))?;
                    }

                    // 提交事务
                    tx.commit().await.map_err(|e| format!("提交事务失败: {}", e))?;

                    Ok(())
                }.await;

                match craft_result {
                    Ok(()) => {
                        let event = WorldEvent {
                            event_type: "action_result".to_string(),
                            tick_id,
                            description: format!("你制造了 {} 个 {}", quantity, item_id),
                            metadata: serde_json::json!({
                                "action": "craft",
                                "item_id": item_id,
                                "quantity": quantity,
                            }),
                        };
                        events.push((*agent_id, event));
                        true // 制造成功
                    }
                    Err(e) => {
                        warn!("制造失败: {}", e);
                        let event = WorldEvent {
                            event_type: "action_result".to_string(),
                            tick_id,
                            description: format!("制造失败: {}", e),
                            metadata: serde_json::json!({
                                "action": "craft_failed",
                                "reason": e,
                            }),
                        };
                        events.push((*agent_id, event));
                        false // 制造失败
                    }
                }
            } else {
                // 没有找到配方
                let event = WorldEvent {
                    event_type: "action_result".to_string(),
                    tick_id,
                    description: format!("制造失败: 找不到 {} 的配方", item_id),
                    metadata: serde_json::json!({
                        "action": "craft_failed",
                        "reason": "recipe_not_found",
                    }),
                };
                events.push((*agent_id, event));
                false // 配方未找到
            }
        }
        StateChange::ItemEquipped { agent_id, item_id } => {
            let result = if let Err(e) =
                crate::inventory::InventoryManager::equip_item(db_pool, *agent_id, item_id).await
            {
                warn!("装备物品失败: {}", e);
                false // 装备失败
            } else {
                // 装备物品通常不产生事件
                true // 装备成功
            };
            result
        }
        StateChange::MessageSpoken { agent_id, content } => {
            tracing::info!("Agent {}: {}", agent_id, content);

            // 找到说话者的位置
            let location = agent_states.iter()
                .find(|s| s.agent_id == *agent_id)
                .map(|s| s.node_id.clone());

            if let Some(node_id) = location {
                // 遍历所有 Agent，如果在同一位置，则添加事件
                for state in agent_states.iter() {
                    // 只有同场景且存活的 Agent 能听到（包括自己，作为确认）
                    if state.node_id == node_id && state.is_alive {
                        let event = WorldEvent {
                            event_type: "public_message".to_string(),
                            tick_id,
                            description: format!("有人说: {}", content),
                            metadata: serde_json::json!({
                                "from_agent_id": agent_id,
                                "content": content,
                                "channel": "local",
                            }),
                        };
                        events.push((state.agent_id, event));
                    }
                }
            }
            true // 说话始终成功（内存操作）
        }
        StateChange::AgentDied { agent_id, cause } => {
            if let Some(state) = agent_states.iter_mut().find(|s| s.agent_id == *agent_id) {
                let was_alive = state.is_alive;
                state.is_alive = false;
                warn!("Agent {} 死亡: {}", agent_id, cause);

                if was_alive && !state.inventory_cleared_this_tick {
                    state.inventory_cleared_this_tick = true;
                    let location = state.node_id.clone();
                    match crate::inventory::InventoryManager::clear_inventory(db_pool, *agent_id).await {
                        Ok(items) => {
                            for item in items {
                                if let Err(e) = crate::db::add_ground_item(db_pool, &location, &item.item_id, item.quantity, Some(*agent_id)).await {
                                    warn!("死亡掉落物品添加到地面失败: {}", e);
                                }
                            }
                        }
                        Err(e) => warn!("清空死亡Agent {} 背包失败: {}", agent_id, e),
                    }
                }

                // 记录死亡事件
                let event = WorldEvent {
                    event_type: "action_result".to_string(),
                    tick_id,
                    description: format!("你因 {} 而死亡", cause),
                    metadata: serde_json::json!({
                        "cause": "death",
                        "reason": cause,
                    }),
                };
                events.push((*agent_id, event));
            }
            true // 死亡处理始终成功（内存操作）
        }
        StateChange::ItemDropped {
            from_agent,
            item_id,
            quantity,
            location,
        } => {
            // 物品掉落：从背包移除并添加到地面
            let result = if let Err(e) = crate::inventory::InventoryManager::remove_item(
                db_pool,
                *from_agent,
                item_id,
                *quantity,
            )
            .await
            {
                warn!("掉落物品失败（背包扣除失败）: {}", e);
                false // 掉落失败
            } else {
                // 添加到地面
                if let Err(e) = crate::db::add_ground_item(db_pool, location, item_id, *quantity, Some(*from_agent)).await {
                    warn!("掉落物品添加到地面失败: {}", e);
                }

                // 记录物品掉落事件
                let event = WorldEvent {
                    event_type: "action_result".to_string(),
                    tick_id,
                    description: format!("你掉落了 {} 个 {}", quantity, item_id),
                    metadata: serde_json::json!({
                        "action": "drop",
                        "item_id": item_id,
                        "quantity": quantity,
                        "location": location,
                    }),
                };
                events.push((*from_agent, event));
                true // 掉落成功
            };
            result
        }
        StateChange::LocationChanged {
            agent_id,
            old_location,
            new_location,
        } => {
            // 更新内存中的 agent_state（当前 tick 广播使用）
            if let Some(state) = agent_states.iter_mut().find(|s| s.agent_id == *agent_id) {
                state.node_id = new_location.clone();
            }

            // 更新数据库
            let result = if let Err(e) = crate::db::update_agent_location(db_pool, *agent_id, new_location).await
            {
                warn!("更新位置失败: {}", e);
                false // 更新失败
            } else {
                // 记录位置变更事件
                let event = WorldEvent {
                    event_type: "action_result".to_string(),
                    tick_id,
                    description: format!("你从 {} 移动到了 {}", old_location, new_location),
                    metadata: serde_json::json!({
                        "action": "move",
                        "old_location": old_location,
                        "new_location": new_location,
                    }),
                };
                events.push((*agent_id, event));
                true // 更新成功
            };
            result
        }
    }
}
