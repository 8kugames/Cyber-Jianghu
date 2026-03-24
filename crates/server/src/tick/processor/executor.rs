//! 状态变更执行器
//!
//! 处理 mutator 未覆盖的状态变更类型，提供回退执行逻辑。

use tracing::warn;

use crate::actions::StateChange;
use crate::db::DbPool;
use crate::models::{AgentState, WorldEvent};

/// 应用状态变更的回退逻辑
///
/// 用于处理 mutator 未覆盖的状态变更类型
pub async fn apply_state_change(
    db_pool: &DbPool,
    tick_id: i64,
    change: &StateChange,
    intent_id: Option<uuid::Uuid>,
    agent_states: &mut [AgentState],
    events: &mut Vec<(uuid::Uuid, WorldEvent)>,
) -> bool {
    match change {
        StateChange::HungerChanged { agent_id, delta } => {
            if let Some(state) = agent_states.iter_mut().find(|s| s.agent_id == *agent_id) {
                let context = state.get_formula_context();
                let _ = state.status.apply_change("hunger", *delta, &context);
                if state.status.check_death_condition("hunger") {
                    state.is_alive = false;
                    let _ = state.status.set("hp", 0);
                    tracing::warn!("Agent {} 因饥饿归零而死亡 (Tick: {})", agent_id, tick_id);
                }
            }
            true
        }
        StateChange::ThirstChanged { agent_id, delta } => {
            if let Some(state) = agent_states.iter_mut().find(|s| s.agent_id == *agent_id) {
                let context = state.get_formula_context();
                let _ = state.status.apply_change("thirst", *delta, &context);
                if state.status.check_death_condition("thirst") {
                    state.is_alive = false;
                    let _ = state.status.set("hp", 0);
                    tracing::warn!("Agent {} 因口渴归零而死亡 (Tick: {})", agent_id, tick_id);
                }
            }
            true
        }
        StateChange::StaminaChanged { agent_id, delta } => {
            if let Some(state) = agent_states.iter_mut().find(|s| s.agent_id == *agent_id) {
                let context = state.get_formula_context();
                let _ = state.status.apply_change("stamina", *delta, &context);
                if state.status.check_death_condition("stamina") {
                    state.is_alive = false;
                    let _ = state.status.set("hp", 0);
                    tracing::warn!("Agent {} 因体力归零而死亡 (Tick: {})", agent_id, tick_id);
                }
            }
            true
        }
        StateChange::ItemTransferred {
            from,
            to,
            item_id,
            quantity,
        } => {
            let result = crate::inventory::InventoryManager::transfer_item(
                db_pool, *from, *to, item_id, *quantity,
            )
            .await;

            if let Err(e) = result {
                warn!("物品转移失败: {}", e);
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
                false
            } else {
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
                true
            }
        }
        StateChange::ItemUsed {
            agent_id,
            item_id,
            effects,
        } => {
            let remove_result =
                crate::inventory::InventoryManager::remove_item(db_pool, *agent_id, item_id, 1)
                    .await;

            if let Err(e) = remove_result {
                warn!(
                    "移除物品失败（物品不存在或数量不足）: agent={}, item={}, error={}",
                    agent_id, item_id, e
                );
                let event = WorldEvent {
                    event_type: "action_result".to_string(),
                    tick_id,
                    description: format!("使用失败，你没有 {}", item_id),
                    metadata: serde_json::json!({
                        "action": "use",
                        "item_id": item_id,
                        "intent_id": intent_id,
                        "result": "failed",
                        "reason": if e.to_string().contains("不足") { "insufficient_quantity" } else { "item_not_found" },
                    }),
                };
                events.push((*agent_id, event));
                false
            } else {
                let mut attribute_delta = serde_json::Map::new();
                if let Some(state) = agent_states.iter_mut().find(|s| s.agent_id == *agent_id) {
                    let context = state.get_formula_context();
                    for effect in effects {
                        let value_to_apply = match effect.operator.as_str() {
                            "set" => {
                                if let Ok(current_value) = state
                                    .status
                                    .collection
                                    .attributes
                                    .get(&effect.attribute)
                                    .map(|attr| attr.value.get())
                                    .ok_or("attribute_not_found")
                                {
                                    effect.value - current_value
                                } else {
                                    effect.value
                                }
                            }
                            "multiply" => {
                                if let Ok(current_value) = state
                                    .status
                                    .collection
                                    .attributes
                                    .get(&effect.attribute)
                                    .map(|attr| attr.value.get())
                                    .ok_or("attribute_not_found")
                                {
                                    (current_value * effect.value) - current_value
                                } else {
                                    effect.value
                                }
                            }
                            _ => effect.value,
                        };

                        if state
                                .status
                                .apply_change(&effect.attribute, value_to_apply, &context).is_ok()
                        {
                            attribute_delta.insert(
                                effect.attribute.clone(),
                                serde_json::json!(value_to_apply),
                            );
                        }
                    }

                    if state.status.check_death_condition("hp") {
                        state.is_alive = false;
                        let _ = state.status.set("hp", 0);
                        tracing::warn!("Agent {} 因HP归零而死亡 (Tick: {})", agent_id, tick_id);
                    }
                }

                let event = WorldEvent {
                    event_type: "action_result".to_string(),
                    tick_id,
                    description: format!("你使用了 {}", item_id),
                    metadata: serde_json::json!({
                        "action": "use",
                        "item_id": item_id,
                        "intent_id": intent_id,
                        "result": "success",
                        "inventory_delta": { item_id: -1 },
                        "attribute_delta": attribute_delta,
                    }),
                };
                events.push((*agent_id, event));
                true
            }
        }
        StateChange::ItemPickedUp {
            agent_id,
            item_id,
            quantity,
        } => {
            if let Some(state) = agent_states.iter().find(|s| s.agent_id == *agent_id) {
                let node_id = state.node_id.clone();

                match crate::db::remove_ground_item(db_pool, &node_id, item_id, *quantity).await {
                    Ok(true) => {
                        if let Err(e) = crate::inventory::InventoryManager::add_item(
                            db_pool, *agent_id, item_id, *quantity,
                        )
                        .await
                        {
                            warn!("拾取物品添加到背包失败: {}，尝试放回地面", e);
                            if let Err(rollback_e) = crate::db::add_ground_item(
                                db_pool, &node_id, item_id, *quantity, None,
                            )
                            .await
                            {
                                warn!("严重错误: 物品放回地面失败，物品丢失: {}", rollback_e);
                            }

                            let event = WorldEvent {
                                event_type: "action_result".to_string(),
                                tick_id,
                                description: "拾取失败，背包已满或发生错误".to_string(),
                                metadata: serde_json::json!({
                                    "action": "pickup_failed",
                                    "item_id": item_id,
                                    "reason": e.to_string(),
                                }),
                            };
                            events.push((*agent_id, event));
                            false
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
                            true
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
                        false
                    }
                }
            } else {
                false
            }
        }
        StateChange::ItemGathered {
            agent_id,
            item_id,
            quantity,
        } => {
            if let Err(e) =
                crate::inventory::InventoryManager::add_item(db_pool, *agent_id, item_id, *quantity)
                    .await
            {
                warn!("采集物品失败: {}", e);
                false
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
                true
            }
        }
        StateChange::ItemCrafted {
            agent_id,
            item_id,
            quantity,
        } => {
            let recipe_to_craft = {
                let cache = crate::game_data::registry_or_panic().get();
                cache
                    .recipes
                    .data
                    .values()
                    .find(|r| r.result_item == *item_id)
                    .cloned()
            };

            if let Some(recipe) = recipe_to_craft {
                let craft_result = async {
                    let mut tx = match db_pool.begin().await {
                        Ok(tx) => tx,
                        Err(e) => return Err(format!("无法开启事务: {}", e)),
                    };

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
                            return Err(format!(
                                "材料不足: {} (需要 {}, 拥有 {})",
                                mat.item_id, mat.quantity, count
                            ));
                        }
                    }

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
                            sqlx::query(
                                "DELETE FROM agent_inventory WHERE agent_id = $1 AND item_id = $2",
                            )
                            .bind(*agent_id)
                            .bind(&mat.item_id)
                            .execute(&mut *tx)
                            .await
                            .map_err(|e| format!("删除材料失败: {}", e))?;
                        } else {
                            sqlx::query(
                                "UPDATE agent_inventory SET quantity = $1 WHERE agent_id = $2 AND item_id = $3",
                            )
                            .bind(new_qty)
                            .bind(*agent_id)
                            .bind(&mat.item_id)
                            .execute(&mut *tx)
                            .await
                            .map_err(|e| format!("更新材料数量失败: {}", e))?;
                        }
                    }

                    let existing: Option<i32> = sqlx::query_scalar(
                        "SELECT quantity FROM agent_inventory WHERE agent_id = $1 AND item_id = $2",
                    )
                    .bind(*agent_id)
                    .bind(item_id)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(|e| format!("查询成品失败: {}", e))?;

                    if let Some(qty) = existing {
                        sqlx::query(
                            "UPDATE agent_inventory SET quantity = $1 WHERE agent_id = $2 AND item_id = $3",
                        )
                        .bind(qty + quantity)
                        .bind(*agent_id)
                        .bind(item_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| format!("更新成品数量失败: {}", e))?;
                    } else {
                        let slot_count: i64 = sqlx::query_scalar(
                            "SELECT COUNT(*) FROM agent_inventory WHERE agent_id = $1",
                        )
                        .bind(*agent_id)
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(|e| format!("检查背包格子失败: {}", e))?;

                        if slot_count >= crate::inventory::get_max_slots() as i64 {
                            return Err("背包已满".to_string());
                        }

                        sqlx::query(
                            "INSERT INTO agent_inventory (agent_id, item_id, quantity, is_equipped) VALUES ($1, $2, $3, false)",
                        )
                        .bind(*agent_id)
                        .bind(item_id)
                        .bind(*quantity)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| format!("插入成品失败: {}", e))?;
                    }

                    tx.commit()
                        .await
                        .map_err(|e| format!("提交事务失败: {}", e))?;

                    Ok(())
                }
                .await;

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
                        true
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
                        false
                    }
                }
            } else {
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
                false
            }
        }
        StateChange::ItemEquipped { agent_id, item_id } => {
            if let Err(e) =
                crate::inventory::InventoryManager::equip_item(db_pool, *agent_id, item_id).await
            {
                warn!("装备物品失败: {}", e);
                false
            } else {
                true
            }
        }
        StateChange::MessageSpoken { agent_id, content } => {
            tracing::info!("Agent {}: {}", agent_id, content);

            let location = agent_states
                .iter()
                .find(|s| s.agent_id == *agent_id)
                .map(|s| s.node_id.clone());

            if let Some(node_id) = location {
                for state in agent_states.iter() {
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
            true
        }
        StateChange::AgentDied { agent_id, cause } => {
            if let Some(state) = agent_states.iter_mut().find(|s| s.agent_id == *agent_id) {
                let was_alive = state.is_alive;
                state.is_alive = false;
                warn!("Agent {} 死亡: {}", agent_id, cause);

                if was_alive && !state.inventory_cleared_this_tick {
                    state.inventory_cleared_this_tick = true;
                    let location = state.node_id.clone();
                    match crate::inventory::InventoryManager::clear_inventory(db_pool, *agent_id)
                        .await
                    {
                        Ok(items) => {
                            for item in items {
                                if let Err(e) = crate::db::add_ground_item(
                                    db_pool,
                                    &location,
                                    &item.item_id,
                                    item.quantity,
                                    Some(*agent_id),
                                )
                                .await
                                {
                                    warn!("死亡掉落物品添加到地面失败: {}", e);
                                }
                            }
                        }
                        Err(e) => warn!("清空死亡Agent {} 背包失败: {}", agent_id, e),
                    }
                }

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
            true
        }
        StateChange::ItemDropped {
            from_agent,
            item_id,
            quantity,
            location,
        } => {
            if let Err(e) = crate::inventory::InventoryManager::remove_item(
                db_pool,
                *from_agent,
                item_id,
                *quantity,
            )
            .await
            {
                warn!("掉落物品失败（背包扣除失败）: {}", e);
                false
            } else {
                if let Err(e) = crate::db::add_ground_item(
                    db_pool,
                    location,
                    item_id,
                    *quantity,
                    Some(*from_agent),
                )
                .await
                {
                    warn!("掉落物品添加到地面失败: {}", e);
                }

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
                true
            }
        }
        StateChange::TradeExecuted {
            initiator,
            target,
            item_id,
            item_quantity,
            price,
        } => {
            let result = async {
                let mut tx = match db_pool.begin().await {
                    Ok(tx) => tx,
                    Err(e) => {
                        warn!("交易失败：无法开启事务: {}", e);
                        return Err("交易失败：数据库错误".to_string());
                    }
                };

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
                    return Err(format!(
                        "物品数量不足: 需要 {}, 拥有 {}",
                        item_quantity, available
                    ));
                }

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
                    return Err(format!(
                        "银两不足: 需要 {}, 拥有 {}",
                        price, silver_available
                    ));
                }

                if *price > 0 {
                    if silver_available == *price {
                        sqlx::query(
                            "DELETE FROM agent_inventory WHERE agent_id = $1 AND item_id = 'silver'",
                        )
                        .bind(*target)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| format!("扣除银两失败: {}", e))?;
                    } else {
                        sqlx::query(
                            "UPDATE agent_inventory SET quantity = $1 WHERE agent_id = $2 AND item_id = 'silver'",
                        )
                        .bind(silver_available - price)
                        .bind(*target)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| format!("更新银两数量失败: {}", e))?;
                    }
                }

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
                    sqlx::query(
                        "UPDATE agent_inventory SET quantity = $1 WHERE agent_id = $2 AND item_id = $3",
                    )
                    .bind(qty + item_quantity)
                    .bind(*target)
                    .bind(item_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| format!("给目标添加物品失败: {}", e))?;
                } else {
                    let slot_count: i64 = sqlx::query_scalar(
                        "SELECT COUNT(*) FROM agent_inventory WHERE agent_id = $1",
                    )
                    .bind(*target)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|e| format!("检查目标背包失败: {}", e))?;

                    if slot_count >= crate::inventory::get_max_slots() as i64 {
                        return Err("目标背包已满".to_string());
                    }

                    sqlx::query(
                        "INSERT INTO agent_inventory (agent_id, item_id, quantity, is_equipped) VALUES ($1, $2, $3, false)",
                    )
                    .bind(*target)
                    .bind(item_id)
                    .bind(item_quantity)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| format!("给目标插入物品失败: {}", e))?;
                }

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
                        sqlx::query(
                            "UPDATE agent_inventory SET quantity = $1 WHERE agent_id = $2 AND item_id = 'silver'",
                        )
                        .bind(qty + price)
                        .bind(*initiator)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| format!("给发起者添加银两失败: {}", e))?;
                    } else {
                        sqlx::query(
                            "INSERT INTO agent_inventory (agent_id, item_id, quantity, is_equipped) VALUES ($1, $2, $3, false)",
                        )
                        .bind(*initiator)
                        .bind("silver")
                        .bind(price)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| format!("给发起者插入银两失败: {}", e))?;
                    }
                }

                tx.commit()
                    .await
                    .map_err(|e| format!("提交事务失败: {}", e))?;

                Ok(())
            }
            .await;

            match result {
                Ok(()) => {
                    let initiator_event = WorldEvent {
                        event_type: "action_result".to_string(),
                        tick_id,
                        description: format!(
                            "你以 {} 两银子出售了 {} 个 {}",
                            price, item_quantity, item_id
                        ),
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
                        description: format!(
                            "你以 {} 两银子购买了 {} 个 {}",
                            price, item_quantity, item_id
                        ),
                        metadata: serde_json::json!({
                            "action": "trade",
                            "from": initiator.to_string(),
                            "item_id": item_id,
                            "quantity": item_quantity,
                            "price": price,
                        }),
                    };
                    events.push((*target, target_event));
                    true
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
                    false
                }
            }
        }
        StateChange::LocationChanged {
            agent_id,
            old_location,
            new_location,
        } => {
            if let Some(state) = agent_states.iter_mut().find(|s| s.agent_id == *agent_id) {
                state.node_id = new_location.clone();
            }

            if let Err(e) = crate::db::update_agent_location(db_pool, *agent_id, new_location).await
            {
                warn!("更新位置失败: {}", e);
                false
            } else {
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
                true
            }
        }
        // AttributeChanged, HpChanged 由 AttributeMutator 处理
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hunger_changed() {
        let db_pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://postgres@localhost/postgres")
            .unwrap();
        let tick_id = 1i64;
        let agent_id = uuid::Uuid::new_v4();
        let agent_state = AgentState::new(agent_id, tick_id);
        let mut events = Vec::new();

        let change = StateChange::HungerChanged {
            agent_id,
            delta: 10,
        };

        let result = apply_state_change(
            &db_pool,
            tick_id,
            &change,
            None,
            &mut [agent_state],
            &mut events,
        )
        .await;

        assert!(result);
    }
}
