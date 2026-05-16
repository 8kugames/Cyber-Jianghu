// ============================================================================
// 角色身份绑定数据库操作
// ============================================================================

use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::Postgres;
use uuid::Uuid;

use super::DbPool;

/// Agent 角色身份绑定
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct AgentRole {
    pub agent_id: Uuid,
    pub role_key: String,
    pub assigned_at: DateTime<Utc>,
}

/// 获取指定 Agent 的角色身份列表
pub async fn get_agent_roles(pool: &DbPool, agent_id: Uuid) -> Result<Vec<AgentRole>> {
    let rows = sqlx::query_as::<Postgres, AgentRole>(
        "SELECT agent_id, role_key, assigned_at \
         FROM agent_assigned_roles WHERE agent_id = $1 ORDER BY role_key",
    )
    .bind(agent_id)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// 分配角色身份（幂等：重复授予返回已有记录）
pub async fn assign_role(pool: &DbPool, agent_id: Uuid, role_key: &str) -> Result<AgentRole> {
    let row = sqlx::query_as::<Postgres, AgentRole>(
        "INSERT INTO agent_assigned_roles (agent_id, role_key) \
         VALUES ($1, $2) \
         ON CONFLICT (agent_id, role_key) DO UPDATE SET assigned_at = NOW() \
         RETURNING agent_id, role_key, assigned_at",
    )
    .bind(agent_id)
    .bind(role_key)
    .fetch_one(pool)
    .await?;

    Ok(row)
}

/// 移除角色身份
pub async fn remove_role(pool: &DbPool, agent_id: Uuid, role_key: &str) -> Result<bool> {
    let result =
        sqlx::query("DELETE FROM agent_assigned_roles WHERE agent_id = $1 AND role_key = $2")
            .bind(agent_id)
            .bind(role_key)
            .execute(pool)
            .await?;

    Ok(result.rows_affected() > 0)
}
