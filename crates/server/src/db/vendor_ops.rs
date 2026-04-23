// ============================================================================
// Vendor 补货规则数据库操作
// ============================================================================

use anyhow::Result;
use sqlx::Postgres;
use uuid::Uuid;

use super::DbPool;

/// 补货规则
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct VendorRefillRule {
    pub agent_id: Uuid,
    pub item_id: String,
    pub threshold: i32,
    pub refill_to: i32,
    pub budget_ratio: i32,
    pub enabled: bool,
}

/// 获取指定 Agent 的补货规则
pub async fn get_vendor_refills(pool: &DbPool, agent_id: Uuid) -> Result<Vec<VendorRefillRule>> {
    let rows = sqlx::query_as::<Postgres, VendorRefillRule>(
        "SELECT agent_id, item_id, threshold, refill_to, budget_ratio, enabled \
         FROM agent_vendor_refill WHERE agent_id = $1 ORDER BY item_id",
    )
    .bind(agent_id)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// 获取所有启用的补货规则（scheduler 用）
pub async fn get_all_enabled_vendor_refills(pool: &DbPool) -> Result<Vec<VendorRefillRule>> {
    let rows = sqlx::query_as::<Postgres, VendorRefillRule>(
        "SELECT agent_id, item_id, threshold, refill_to, budget_ratio, enabled \
         FROM agent_vendor_refill WHERE enabled = true",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// 设置/更新补货规则（UPSERT）
pub async fn set_vendor_refill(
    pool: &DbPool,
    agent_id: Uuid,
    item_id: &str,
    threshold: i32,
    refill_to: i32,
    budget_ratio: i32,
) -> Result<VendorRefillRule> {
    let row = sqlx::query_as::<Postgres, VendorRefillRule>(
        "INSERT INTO agent_vendor_refill (agent_id, item_id, threshold, refill_to, budget_ratio) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (agent_id, item_id) \
         DO UPDATE SET threshold = EXCLUDED.threshold, \
                       refill_to = EXCLUDED.refill_to, \
                       budget_ratio = EXCLUDED.budget_ratio, \
                       enabled = true, \
                       updated_at = NOW() \
         RETURNING agent_id, item_id, threshold, refill_to, budget_ratio, enabled",
    )
    .bind(agent_id)
    .bind(item_id)
    .bind(threshold)
    .bind(refill_to)
    .bind(budget_ratio)
    .fetch_one(pool)
    .await?;

    Ok(row)
}

/// 删除补货规则
pub async fn remove_vendor_refill(pool: &DbPool, agent_id: Uuid, item_id: &str) -> Result<bool> {
    let result =
        sqlx::query("DELETE FROM agent_vendor_refill WHERE agent_id = $1 AND item_id = $2")
            .bind(agent_id)
            .bind(item_id)
            .execute(pool)
            .await?;

    Ok(result.rows_affected() > 0)
}

/// 切换补货规则启用状态
pub async fn toggle_vendor_refill(
    pool: &DbPool,
    agent_id: Uuid,
    item_id: &str,
    enabled: bool,
) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE agent_vendor_refill SET enabled = $3, updated_at = NOW() \
         WHERE agent_id = $1 AND item_id = $2",
    )
    .bind(agent_id)
    .bind(item_id)
    .bind(enabled)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}
