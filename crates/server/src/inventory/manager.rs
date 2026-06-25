// ============================================================================
// OpenClaw Cyber-Jianghu 背包管理器
// ============================================================================

use sqlx::PgPool;
use std::collections::HashMap;
use tracing::{debug, info};
use uuid::Uuid;

use super::error::InventoryError;
use super::types::{InventoryItem, get_max_slots};

/// 背包管理器
pub struct InventoryManager;

impl InventoryManager {
    /// 添加物品到Agent背包
    ///
    /// 如果物品已存在，增加数量；否则创建新记录
    pub async fn add_item(
        conn: &mut sqlx::PgConnection,
        agent_id: Uuid,
        item_id: &str,
        quantity: i32,
    ) -> Result<(), InventoryError> {
        debug!(
            "添加物品: agent={}, item={}, qty={}",
            agent_id, item_id, quantity
        );

        // 检查是否已有该物品
        let existing = sqlx::query_as::<_, InventoryItem>(
            "SELECT id, agent_id, item_id, quantity, is_equipped FROM agent_inventory WHERE agent_id = $1 AND item_id = $2"
        )
        .bind(agent_id)
        .bind(item_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| InventoryError::DatabaseError(e.to_string()))?;

        if let Some(item) = existing {
            // 已存在，检查堆叠上限
            let new_quantity = item.quantity + quantity;
            // 优先使用 items.yaml 中 per-item stack_size，fallback 到 inventory.yaml 全局上限
            let max_stack_size = crate::game_data::ItemRegistry::get(item_id)
                .map(|cfg| cfg.stack_size)
                .unwrap_or_else(crate::inventory::types::get_max_stack_size);

            if new_quantity > max_stack_size {
                return Err(InventoryError::StackLimitExceeded {
                    item_id: item_id.to_string(),
                    current: item.quantity,
                    requested: quantity,
                    max: max_stack_size,
                });
            }

            sqlx::query("UPDATE agent_inventory SET quantity = $1 WHERE id = $2")
                .bind(new_quantity)
                .bind(item.id)
                .execute(&mut *conn)
                .await
                .map_err(|e| InventoryError::DatabaseError(e.to_string()))?;

            info!("更新物品数量: {} x{}", item_id, new_quantity);
        } else {
            // 不存在，检查背包是否有空位
            let slot_count = Self::get_slot_count(&mut *conn, agent_id).await?;

            if slot_count >= get_max_slots() {
                // 检查是否可以堆叠到现有物品
                return Err(InventoryError::InventoryFull);
            }

            // 创建新物品记录
            sqlx::query(
                "INSERT INTO agent_inventory (agent_id, item_id, quantity, is_equipped) VALUES ($1, $2, $3, false)"
            )
            .bind(agent_id)
            .bind(item_id)
            .bind(quantity)
            .execute(&mut *conn)
            .await
            .map_err(|e| InventoryError::DatabaseError(e.to_string()))?;

            info!("添加新物品: {} x{}", item_id, quantity);
        }

        Ok(())
    }

    /// 从Agent背包移除物品
    ///
    /// 如果数量不足，返回错误
    pub async fn remove_item(
        conn: &mut sqlx::PgConnection,
        agent_id: Uuid,
        item_id: &str,
        quantity: i32,
    ) -> Result<(), InventoryError> {
        debug!(
            "移除物品: agent={}, item={}, qty={}",
            agent_id, item_id, quantity
        );

        let item = sqlx::query_as::<_, InventoryItem>(
            "SELECT id, agent_id, item_id, quantity, is_equipped FROM agent_inventory WHERE agent_id = $1 AND item_id = $2"
        )
        .bind(agent_id)
        .bind(item_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| InventoryError::DatabaseError(e.to_string()))?
        .ok_or_else(|| InventoryError::ItemNotFound(item_id.to_string()))?;

        if item.quantity < quantity {
            return Err(InventoryError::InsufficientQuantity {
                required: quantity,
                available: item.quantity,
            });
        }

        let new_quantity = item.quantity - quantity;

        if new_quantity == 0 {
            // 数量为0，删除记录
            sqlx::query("DELETE FROM agent_inventory WHERE id = $1")
                .bind(item.id)
                .execute(&mut *conn)
                .await
                .map_err(|e| InventoryError::DatabaseError(e.to_string()))?;

            info!("删除物品: {}", item_id);
        } else {
            // 更新数量
            sqlx::query("UPDATE agent_inventory SET quantity = $1 WHERE id = $2")
                .bind(new_quantity)
                .bind(item.id)
                .execute(&mut *conn)
                .await
                .map_err(|e| InventoryError::DatabaseError(e.to_string()))?;

            info!("更新物品数量: {} x{}", item_id, new_quantity);
        }

        Ok(())
    }

    /// 获取Agent背包占用的格子数
    pub async fn get_slot_count(conn: &mut sqlx::PgConnection, agent_id: Uuid) -> Result<i32, InventoryError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM agent_inventory WHERE agent_id = $1",
        )
        .bind(agent_id)
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| InventoryError::DatabaseError(e.to_string()))?;

        Ok(count as i32)
    }

    /// 获取Agent的所有背包物品
    pub async fn get_all_items(
        pool: &PgPool,
        agent_id: Uuid,
    ) -> Result<Vec<InventoryItem>, InventoryError> {
        let items = sqlx::query_as::<_, InventoryItem>(
            "SELECT id, agent_id, item_id, quantity, is_equipped FROM agent_inventory WHERE agent_id = $1"
        )
        .bind(agent_id)
        .fetch_all(pool)
        .await
        .map_err(|e| InventoryError::DatabaseError(e.to_string()))?;

        Ok(items)
    }

    /// 批量获取多个 Agent 的背包物品（单次 DB 查询，解决 N+1 问题）
    pub async fn get_all_items_batch(
        pool: &PgPool,
        agent_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, Vec<InventoryItem>>, InventoryError> {
        if agent_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let items = sqlx::query_as::<_, InventoryItem>(
            "SELECT id, agent_id, item_id, quantity, is_equipped FROM agent_inventory WHERE agent_id = ANY($1)"
        )
        .bind(agent_ids)
        .fetch_all(pool)
        .await
        .map_err(|e| InventoryError::DatabaseError(e.to_string()))?;

        let mut map: HashMap<Uuid, Vec<InventoryItem>> = HashMap::new();
        for item in items {
            map.entry(item.agent_id).or_default().push(item);
        }
        Ok(map)
    }

    /// 转移物品（give 动作的核心）
    ///
    /// 从一个Agent转移到另一个Agent
    /// 使用事务保证原子性：要么全部成功，要么全部失败
    pub async fn transfer_item(
        conn: &mut sqlx::PgConnection,
        from_agent: Uuid,
        to_agent: Uuid,
        item_id: &str,
        quantity: i32,
    ) -> Result<(), InventoryError> {
        debug!(
            "转移物品: {} -> {}, item={}, qty={}",
            from_agent, to_agent, item_id, quantity
        );

        // 1. 检查来源是否有足够物品（使用 FOR UPDATE 锁定行）
        let available: Option<i32> = sqlx::query_scalar(
            "SELECT quantity FROM agent_inventory WHERE agent_id = $1 AND item_id = $2 FOR UPDATE",
        )
        .bind(from_agent)
        .bind(item_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| InventoryError::DatabaseError(format!("Failed to query item: {}", e)))?;

        let available = available.unwrap_or(0);
        if available < quantity {
            return Err(InventoryError::InsufficientQuantity {
                required: quantity,
                available,
            });
        }

        // 2. 从来源移除物品
        let new_quantity = available - quantity;
        if new_quantity == 0 {
            sqlx::query("DELETE FROM agent_inventory WHERE agent_id = $1 AND item_id = $2")
                .bind(from_agent)
                .bind(item_id)
                .execute(&mut *conn)
                .await
                .map_err(|e| {
                    InventoryError::DatabaseError(format!("Failed to delete item: {}", e))
                })?;
        } else {
            sqlx::query(
                "UPDATE agent_inventory SET quantity = $1 WHERE agent_id = $2 AND item_id = $3",
            )
            .bind(new_quantity)
            .bind(from_agent)
            .bind(item_id)
            .execute(&mut *conn)
            .await
            .map_err(|e| InventoryError::DatabaseError(format!("Failed to update item: {}", e)))?;
        }

        // 3. 添加到目标
        let target_existing: Option<i32> = sqlx::query_scalar(
            "SELECT quantity FROM agent_inventory WHERE agent_id = $1 AND item_id = $2 FOR UPDATE",
        )
        .bind(to_agent)
        .bind(item_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| {
            InventoryError::DatabaseError(format!("Failed to query target item: {}", e))
        })?;

        if let Some(target_qty) = target_existing {
            // 目标已有该物品，检查堆叠上限后增加数量
            let max_stack = crate::game_data::ItemRegistry::get(item_id)
                .map(|cfg| cfg.stack_size)
                .unwrap_or_else(crate::inventory::types::get_max_stack_size);
            let new_qty = target_qty + quantity;
            if new_qty > max_stack {
                return Err(InventoryError::StackLimitExceeded {
                    item_id: item_id.to_string(),
                    current: target_qty,
                    requested: quantity,
                    max: max_stack,
                });
            }
            sqlx::query(
                "UPDATE agent_inventory SET quantity = $1 WHERE agent_id = $2 AND item_id = $3",
            )
            .bind(new_qty)
            .bind(to_agent)
            .bind(item_id)
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                InventoryError::DatabaseError(format!("Failed to update target item: {}", e))
            })?;
        } else {
            // 目标没有该物品，检查槽位上限
            let slot_count = Self::get_slot_count(&mut *conn, to_agent).await?;
            if slot_count >= get_max_slots() {
                return Err(InventoryError::InventoryFull);
            }

            sqlx::query(
                "INSERT INTO agent_inventory (agent_id, item_id, quantity, is_equipped) VALUES ($1, $2, $3, false)",
            )
            .bind(to_agent)
            .bind(item_id)
            .bind(quantity)
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                InventoryError::DatabaseError(format!("Failed to insert target item: {}", e))
            })?;
        }

        info!(
            "物品转移成功: {} x{} 从 {} 到 {}",
            item_id, quantity, from_agent, to_agent
        );
        Ok(())
    }

    /// 装备物品
    pub async fn equip_item(
        conn: &mut sqlx::PgConnection,
        agent_id: Uuid,
        item_id: &str,
    ) -> Result<(), InventoryError> {
        debug!("装备物品: agent={}, item={}", agent_id, item_id);

        let existing = sqlx::query_as::<_, InventoryItem>(
            "SELECT id, agent_id, item_id, quantity, is_equipped FROM agent_inventory WHERE agent_id = $1 AND item_id = $2"
        )
        .bind(agent_id)
        .bind(item_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| InventoryError::DatabaseError(e.to_string()))?;

        if let Some(item) = existing {
            if item.is_equipped {
                return Ok(()); // 已经装备了
            }

            // 当前数据模型只有 item_type，没有细分 equip_slot。
            // 先按大类约束为 weapon/armor，避免继续依赖不存在的配置字段。
            if let Some(config) = crate::game_data::ItemRegistry::get(item_id) {
                if matches!(config.item_type.as_str(), "weapon" | "armor") {
                    sqlx::query("UPDATE agent_inventory SET is_equipped = false WHERE agent_id = $1")
                        .bind(agent_id)
                        .execute(&mut *conn)
                        .await
                        .map_err(|e| InventoryError::DatabaseError(e.to_string()))?;

                    // 装备新物品
                    sqlx::query("UPDATE agent_inventory SET is_equipped = true WHERE id = $1")
                        .bind(item.id)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| InventoryError::DatabaseError(e.to_string()))?;

                    info!("成功装备物品: {}", item_id);
                    Ok(())
                } else {
                    Err(InventoryError::DatabaseError(format!(
                        "物品类型 {} 不支持装备",
                        config.item_type
                    )))
                }
            } else {
                Err(InventoryError::ItemNotFound(item_id.to_string()))
            }
        } else {
            Err(InventoryError::ItemNotFound(item_id.to_string()))
        }
    }

    /// 清空Agent的所有物品（通常用于死亡掉落）
    ///
    /// 返回被清空的物品列表
    pub async fn clear_inventory(
        conn: &mut sqlx::PgConnection,
        agent_id: Uuid,
    ) -> Result<Vec<InventoryItem>, InventoryError> {
        debug!("清空背包: agent={}", agent_id);

        // 获取所有物品
        let items = sqlx::query_as::<_, InventoryItem>(
            "SELECT id, agent_id, item_id, quantity, is_equipped FROM agent_inventory WHERE agent_id = $1"
        )
        .bind(agent_id)
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| InventoryError::DatabaseError(e.to_string()))?;

        if !items.is_empty() {
            sqlx::query("DELETE FROM agent_inventory WHERE agent_id = $1")
                .bind(agent_id)
                .execute(&mut *conn)
                .await
                .map_err(|e| InventoryError::DatabaseError(e.to_string()))?;

            info!("成功清空背包: {} 个物品", items.len());
        }

        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::types::get_max_stack_size;

    #[test]
    fn test_constants() {
        // 初始化测试注册表（数据驱动）
        crate::game_data::init_test_registry();

        // 验证配置访问器返回正确的默认值
        assert_eq!(get_max_slots(), 10);
        assert_eq!(get_max_stack_size(), 10);
    }
}
