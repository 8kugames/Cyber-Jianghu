// ============================================================================
// 地面物品相关数据库操作
// ============================================================================

use anyhow::{Context, Result};
use sqlx::PgPool;
use uuid::Uuid;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct GroundItem {
    pub id: i64,
    pub node_id: String,
    pub item_id: String,
    pub quantity: i32,
    pub dropped_by: Option<Uuid>,
}

/// 添加物品到地面
pub async fn add_ground_item(
    pool: &PgPool,
    node_id: &str,
    item_id: &str,
    quantity: i32,
    dropped_by: Option<Uuid>,
) -> Result<()> {
    let mut tx = pool.begin().await.context("开始事务失败")?;

    // Check if the item already exists on the ground in this node
    let existing: Option<(i64, i32)> = sqlx::query_as(
        r#"
        SELECT id, quantity FROM ground_items 
        WHERE node_id = $1 AND item_id = $2
        FOR UPDATE
        "#,
    )
    .bind(node_id)
    .bind(item_id)
    .fetch_optional(&mut *tx)
    .await
    .context("查询地面物品失败")?;

    if let Some((id, current_quantity)) = existing {
        sqlx::query(
            r#"
            UPDATE ground_items 
            SET quantity = $1, dropped_by = $2 
            WHERE id = $3
            "#,
        )
        .bind(current_quantity + quantity)
        .bind(dropped_by)
        .bind(id)
        .execute(&mut *tx)
        .await
        .context("更新地面物品数量失败")?;
    } else {
        sqlx::query(
            r#"
            INSERT INTO ground_items (node_id, item_id, quantity, dropped_by)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(node_id)
        .bind(item_id)
        .bind(quantity)
        .bind(dropped_by)
        .execute(&mut *tx)
        .await
        .context("添加地面物品失败")?;
    }

    tx.commit().await.context("提交事务失败")?;
    Ok(())
}

/// 从地面移除物品（如果数量归零则删除记录）
pub async fn remove_ground_item(
    pool: &PgPool,
    node_id: &str,
    item_id: &str,
    quantity: i32,
) -> Result<bool> {
    let mut tx = pool.begin().await.context("开始事务失败")?;

    let existing: Option<(i64, i32)> = sqlx::query_as(
        r#"
        SELECT id, quantity FROM ground_items 
        WHERE node_id = $1 AND item_id = $2
        FOR UPDATE
        "#,
    )
    .bind(node_id)
    .bind(item_id)
    .fetch_optional(&mut *tx)
    .await
    .context("查询地面物品失败")?;

    if let Some((id, current_quantity)) = existing {
        if current_quantity > quantity {
            sqlx::query(
                r#"
                UPDATE ground_items 
                SET quantity = $1 
                WHERE id = $2
                "#,
            )
            .bind(current_quantity - quantity)
            .bind(id)
            .execute(&mut *tx)
            .await
            .context("更新地面物品数量失败")?;
        } else if current_quantity == quantity {
            sqlx::query(
                r#"
                DELETE FROM ground_items 
                WHERE id = $1
                "#,
            )
            .bind(id)
            .execute(&mut *tx)
            .await
            .context("删除地面物品记录失败")?;
        } else {
            return Err(anyhow::anyhow!("地面物品数量不足"));
        }
        
        tx.commit().await.context("提交事务失败")?;
        return Ok(true);
    }

    tx.rollback().await.context("回滚事务失败")?;
    Ok(false)
}

/// 获取某个节点的所有地面物品
pub async fn get_ground_items_by_node(
    pool: &PgPool,
    node_id: &str,
) -> Result<Vec<GroundItem>> {
    let items = sqlx::query_as::<_, GroundItem>(
        r#"
        SELECT id, node_id, item_id, quantity, dropped_by
        FROM ground_items
        WHERE node_id = $1
        ORDER BY created_at DESC
        "#,
    )
    .bind(node_id)
    .fetch_all(pool)
    .await
    .context("查询节点地面物品失败")?;

    Ok(items)
}
