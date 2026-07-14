use tracing::{error, warn};

use crate::actions::StateChange;
use crate::models::{AgentState, WorldEvent, WorldEventType};

/// 应用状态变更的回退逻辑
pub async fn apply_state_change(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tick_id: i64,
    change: &StateChange,
    intent_id: Option<uuid::Uuid>,
    agent_states: &mut [AgentState],
    all_states: &[AgentState],
    events: &mut Vec<(uuid::Uuid, WorldEvent)>,
) -> bool {
    match change {
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
                tx, *from, *to, item_id, *quantity,
            )
            .await;

            if let Err(e) = result {
                warn!("物品转移失败: {}", e);
                let event = WorldEvent {
                    event_type: WorldEventType::ActionResult,
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
                let to_name = all_states
                    .iter()
                    .find(|s| s.agent_id == *to)
                    .map(|s| format!("{}（{}）", s.name, to))
                    .unwrap_or_else(|| format!("未知（{}）", to));
                let from_name = all_states
                    .iter()
                    .find(|s| s.agent_id == *from)
                    .map(|s| format!("{}（{}）", s.name, from))
                    .unwrap_or_else(|| format!("未知（{}）", from));

                let event = WorldEvent {
                    event_type: WorldEventType::SocialInteraction,
                    tick_id,
                    description: format!("你给 {} 转移了 {} 个 {}", to_name, quantity, item_id),
                    metadata: serde_json::json!({
                        "action": "转移",
                        "target": to.to_string(),
                        "item_id": item_id,
                        "quantity": quantity,
                    }),
                };
                events.push((*from, event));

                let event = WorldEvent {
                    event_type: WorldEventType::SocialInteraction,
                    tick_id,
                    description: format!("{} 给你转移了 {} 个 {}", from_name, quantity, item_id),
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
                crate::inventory::InventoryManager::remove_item(tx, *agent_id, item_id, 1)
                    .await;

            if let Err(e) = remove_result {
                warn!(
                    "移除物品失败: agent={}, item={}, error={}",
                    agent_id, item_id, e
                );
                let event = WorldEvent {
                    event_type: WorldEventType::ActionResult,
                    tick_id,
                    description: format!("使用失败，你没有 {}", item_id),
                    metadata: serde_json::json!({
                        "action": "使用",
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
                        let value_to_apply = match effect.operation {
                            crate::game_data::Operation::Set => {
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
                            crate::game_data::Operation::Multiply => {
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
                            crate::game_data::Operation::Add => effect.value,
                        };

                        if state
                            .status
                            .apply_change(&effect.attribute, value_to_apply, &context)
                            .is_ok()
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
                    event_type: WorldEventType::ActionResult,
                    tick_id,
                    description: format!("你使用了 {}", item_id),
                    metadata: serde_json::json!({
                        "action": "使用",
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
        StateChange::ItemAcquired {
            agent_id,
            item_id,
            quantity,
            source,
        } => {
            let source_str = source.as_str();
            match source_str {
                "ground" => {
                    if let Some(state) = agent_states.iter().find(|s| s.agent_id == *agent_id) {
                        let node_id = state.node_id.clone();
                        match crate::db::remove_ground_item(tx, &node_id, item_id, *quantity)
                            .await
                        {
                            Ok(true) => {
                                if let Err(e) = crate::inventory::InventoryManager::add_item(
                                    tx, *agent_id, item_id, *quantity,
                                )
                                .await
                                {
                                    warn!("拾取物品添加到背包失败: {}，尝试放回地面", e);
                                    if let Err(rollback_e) = crate::db::add_ground_item(
                                        tx, &node_id, item_id, *quantity, None,
                                    )
                                    .await
                                    {
                                        warn!(
                                            "严重错误: 物品放回地面失败，物品丢失: {}",
                                            rollback_e
                                        );
                                    }
                                    let event = WorldEvent {
                                        event_type: WorldEventType::ActionResult,
                                        tick_id,
                                        description: "拾取失败，背包已满或发生错误".to_string(),
                                        metadata: serde_json::json!({
                                            "action": "acquire_failed",
                                            "item_id": item_id,
                                            "reason": e.to_string(),
                                        }),
                                    };
                                    events.push((*agent_id, event));
                                    false
                                } else {
                                    let event = WorldEvent {
                                        event_type: WorldEventType::ActionResult,
                                        tick_id,
                                        description: format!(
                                            "你拾取了 {} 个 {}",
                                            quantity, item_id
                                        ),
                                        metadata: serde_json::json!({
                                            "action": "取(ground)",
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
                                    event_type: WorldEventType::ActionResult,
                                    tick_id,
                                    description: format!("拾取失败，地面没有 {}", item_id),
                                    metadata: serde_json::json!({
                                        "action": "acquire_failed",
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
                "resource" => {
                    if let Err(e) = crate::inventory::InventoryManager::add_item(
                        tx, *agent_id, item_id, *quantity,
                    )
                    .await
                    {
                        warn!("采集物品失败: {}", e);
                        false
                    } else {
                        let event = WorldEvent {
                            event_type: WorldEventType::ActionResult,
                            tick_id,
                            description: format!("你采集了 {} 个 {}", quantity, item_id),
                            metadata: serde_json::json!({
                                "action": "取(resource)",
                                "item_id": item_id,
                                "quantity": quantity,
                            }),
                        };
                        events.push((*agent_id, event));
                        true
                    }
                }
                "effect" => {
                    if let Err(e) = crate::inventory::InventoryManager::add_item(
                        tx, *agent_id, item_id, *quantity,
                    )
                    .await
                    {
                        warn!("效果添加物品失败: {}", e);
                        false
                    } else {
                        true
                    }
                }
                _ => {
                    warn!("未知的物品来源: {}", source_str);
                    false
                }
            }
        }
        StateChange::ItemDisposed {
            agent_id,
            item_id,
            quantity,
            location,
        } => {
            if let Err(e) = crate::inventory::InventoryManager::remove_item(
                tx, *agent_id, item_id, *quantity,
            )
            .await
            {
                warn!("丢弃物品失败（背包扣除失败）: {}", e);
                false
            } else {
                if let Err(e) = crate::db::add_ground_item(
                    tx,
                    location,
                    item_id,
                    *quantity,
                    Some(*agent_id),
                )
                .await
                {
                    warn!("丢弃物品添加到地面失败，回滚背包: {}", e);
                    if let Err(re) = crate::inventory::InventoryManager::add_item(
                        tx, *agent_id, item_id, *quantity,
                    )
                    .await
                    {
                        error!(
                            "丢弃回滚失败！物品丢失: agent={}, item={}, qty={}, err={}",
                            agent_id, item_id, quantity, re
                        );
                    }
                    return false;
                }

                let event = WorldEvent {
                    event_type: WorldEventType::ActionResult,
                    tick_id,
                    description: format!("你丢弃了 {} 个 {}", quantity, item_id),
                    metadata: serde_json::json!({
                        "action": "予(ground)",
                        "item_id": item_id,
                        "quantity": quantity,
                        "location": location,
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
                let cache = match crate::game_data::registry_or_error() {
                    Ok(r) => r.get(),
                    Err(e) => {
                        tracing::warn!("注册表未初始化，跳过 craft: {}", e);
                        return false;
                    }
                };
                cache
                    .recipes
                    .data
                    .values()
                    .find(|r| r.result_item == *item_id)
                    .cloned()
            };

            if let Some(recipe) = recipe_to_craft {
                let craft_result = async {
                    for mat in &recipe.materials {
                        let count: i32 = sqlx::query_scalar(
                            "SELECT quantity FROM agent_inventory WHERE agent_id = $1 AND item_id = $2 FOR UPDATE",
                        )
                        .bind(*agent_id)
                        .bind(&mat.item_id)
                        .fetch_optional(&mut **tx)
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
                        .fetch_one(&mut **tx)
                        .await
                        .map_err(|e| format!("获取材料数量失败: {}", e))?;

                        let new_qty = current - mat.quantity;
                        if new_qty <= 0 {
                            sqlx::query(
                                "DELETE FROM agent_inventory WHERE agent_id = $1 AND item_id = $2",
                            )
                            .bind(*agent_id)
                            .bind(&mat.item_id)
                            .execute(&mut **tx)
                            .await
                            .map_err(|e| format!("删除材料失败: {}", e))?;
                        } else {
                            sqlx::query(
                                "UPDATE agent_inventory SET quantity = $1 WHERE agent_id = $2 AND item_id = $3",
                            )
                            .bind(new_qty)
                            .bind(*agent_id)
                            .bind(&mat.item_id)
                            .execute(&mut **tx)
                            .await
                            .map_err(|e| format!("更新材料数量失败: {}", e))?;
                        }
                    }

                    let existing: Option<i32> = sqlx::query_scalar(
                        "SELECT quantity FROM agent_inventory WHERE agent_id = $1 AND item_id = $2",
                    )
                    .bind(*agent_id)
                    .bind(item_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(|e| format!("查询成品失败: {}", e))?;

                    if let Some(qty) = existing {
                        sqlx::query(
                            "UPDATE agent_inventory SET quantity = $1 WHERE agent_id = $2 AND item_id = $3",
                        )
                        .bind(qty + quantity)
                        .bind(*agent_id)
                        .bind(item_id)
                        .execute(&mut **tx)
                        .await
                        .map_err(|e| format!("更新成品数量失败: {}", e))?;
                    } else {
                        let slot_count: i64 = sqlx::query_scalar(
                            "SELECT COUNT(*) FROM agent_inventory WHERE agent_id = $1",
                        )
                        .bind(*agent_id)
                        .fetch_one(&mut **tx)
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
                        .execute(&mut **tx)
                        .await
                        .map_err(|e| format!("插入成品失败: {}", e))?;
                    }

                    Ok(())
                }
                .await;

                match craft_result {
                    Ok(()) => {
                        let stamina_cost = recipe.stamina_cost;
                        if let Some(state) =
                            agent_states.iter_mut().find(|s| s.agent_id == *agent_id)
                        {
                            let ctx = state.get_formula_context();
                            let _ = state.status.apply_change("stamina", -stamina_cost, &ctx);
                        }

                        let event = WorldEvent {
                            event_type: WorldEventType::ActionResult,
                            tick_id,
                            description: format!("你制造了 {} 个 {}", quantity, item_id),
                            metadata: serde_json::json!({
                                "action": "制造",
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
                            event_type: WorldEventType::ActionResult,
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
                    event_type: WorldEventType::ActionResult,
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
                crate::inventory::InventoryManager::equip_item(tx, *agent_id, item_id).await
            {
                warn!("装备物品失败: {}", e);
                false
            } else {
                true
            }
        }
        StateChange::MessageSpoken {
            agent_id,
            content,
            channel,
            target_agent_id,
            already_broadcast,
        } => {
            tracing::info!("Agent {} (channel={}): {}", agent_id, channel, content);

            if *already_broadcast {
                return true;
            }

            match channel.as_str() {
                "private" => {
                    // 私密通信：由 handler 的 Dialogue Session 处理
                    // executor 仅确认格式有效
                    true
                }
                "broadcast" => {
                    // 大范围广播：发到所有 agent
                    let mut meta = serde_json::json!({
                        "from_agent_id": agent_id,
                        "content": content,
                        "channel": "broadcast",
                    });
                    if let Some(target) = target_agent_id {
                        meta.as_object_mut()
                            .expect("meta is always a JSON object")
                            .insert("target_agent_id".to_string(), serde_json::json!(target));
                    }
                    for state in agent_states.iter() {
                        if state.is_alive {
                            let event = WorldEvent {
                                event_type: WorldEventType::PublicMessage,
                                tick_id,
                                description: content.clone(),
                                metadata: meta.clone(),
                            };
                            events.push((state.agent_id, event));
                        }
                    }
                    true
                }
                _ => {
                    // public (默认) + fallback：广播到同位置
                    let location = agent_states
                        .iter()
                        .find(|s| s.agent_id == *agent_id)
                        .map(|s| s.node_id.clone());

                    if let Some(node_id) = location {
                        let mut meta = serde_json::json!({
                            "from_agent_id": agent_id,
                            "content": content,
                            "channel": "local",
                            "location": node_id,
                        });
                        if let Some(target) = target_agent_id {
                            meta.as_object_mut()
                                .expect("meta is always a JSON object")
                                .insert("target_agent_id".to_string(), serde_json::json!(target));
                        }
                        for state in agent_states.iter() {
                            if state.node_id == node_id && state.is_alive {
                                let event = WorldEvent {
                                    event_type: WorldEventType::PublicMessage,
                                    tick_id,
                                    description: content.clone(),
                                    metadata: meta.clone(),
                                };
                                events.push((state.agent_id, event));
                            }
                        }
                    }
                    true
                }
            }
        }
        StateChange::AgentDied { agent_id, cause } => {
            if let Some(state) = agent_states.iter_mut().find(|s| s.agent_id == *agent_id) {
                let was_alive = state.is_alive;
                state.is_alive = false;
                warn!("Agent {} 死亡: {}", agent_id, cause);

                if was_alive && !state.inventory_cleared_this_tick {
                    state.inventory_cleared_this_tick = true;
                    let location = state.node_id.clone();
                    match crate::inventory::InventoryManager::clear_inventory(tx, *agent_id)
                        .await
                    {
                        Ok(items) => {
                            for item in items {
                                if let Err(e) = crate::db::add_ground_item(
                                    tx,
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
                    event_type: WorldEventType::ActionResult,
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
        StateChange::LocationChanged {
            agent_id,
            old_location: _,
            new_location,
        } => {
            if let Some(state) = agent_states.iter_mut().find(|s| s.agent_id == *agent_id) {
                state.node_id = new_location.clone();
            }

            if let Err(e) = crate::db::update_agent_location(tx, *agent_id, new_location).await
            {
                warn!("更新位置失败: {}", e);
                false
            } else {
                let event = WorldEvent {
                    event_type: WorldEventType::ActionResult,
                    tick_id,
                    description: format!("你移动到了 {}", new_location),
                    metadata: serde_json::json!({
                        "action": "移动",
                        "new_location": new_location,
                    }),
                };
                events.push((*agent_id, event));
                true
            }
        }
        StateChange::SkillLearned { .. } => true,
        StateChange::RecipeLearned {
            agent_id,
            recipe_id,
            source,
        } => {
            let source_str = source.as_str();
            sqlx::query(
                "INSERT INTO agent_known_recipes (agent_id, recipe_id, learned_at_tick, source)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (agent_id, recipe_id) DO NOTHING",
            )
            .bind(*agent_id)
            .bind(recipe_id)
            .bind(tick_id)
            .bind(source_str)
            .execute(&mut **tx)
            .await
            .is_ok()
        }
        StateChange::Observation {
            observer_id,
            target_id: Some(target_id),
            description,
            detected: true,
        } => {
            if let Some(state) = agent_states.iter().find(|s| s.agent_id == *target_id) {
                let meta = serde_json::json!({
                    "from_agent_id": observer_id,
                    "description": description,
                    "channel": "observation",
                    "location": state.node_id,
                });
                let event = WorldEvent {
                    event_type: WorldEventType::Observation,
                    tick_id,
                    description: description.clone(),
                    metadata: meta,
                };
                events.push((*target_id, event));
            }
            true
        }
        StateChange::Observation { .. } => true,
        _ => true,
    }
}
