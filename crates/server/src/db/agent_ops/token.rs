// ============================================================================
// 设备 token 轮换
// ============================================================================

use super::*;

// ============================================================================
// 设备 token 轮换
// ============================================================================

/// 核心 SQL 集中点：轮换 device 的 auth_token，
/// 同时重置 `token_created_at`、写 `token_rotated_at`。
/// 返回新 token，调用方负责把新凭据传回客户端。
pub(crate) const ROTATE_DEVICE_TOKEN_SQL: &str = r#"
UPDATE devices
SET auth_token = $2,
    token_created_at = NOW(),
    token_rotated_at = NOW()
WHERE device_id = $1
RETURNING auth_token
"#;

/// 轮换 device 的 auth_token 并返回新 token。
///
/// 用途：
/// - `retire_agent` 成功末尾 → 旧凭据立即失效，防御同设备连续创建角色间的会话复用
/// - 显式 rotation 端点（待实现）
/// - 调度器轮换（待接入 config TTL）
pub async fn rotate_device_token(pool: &PgPool, device_id: Uuid) -> Result<String> {
    let new_token = generate_secure_token();
    let row: Option<(String,)> = sqlx::query_as(ROTATE_DEVICE_TOKEN_SQL)
        .bind(device_id)
        .bind(&new_token)
        .fetch_optional(pool)
        .await
        .context("轮换设备 token 失败")?;
    match row {
        Some((token,)) => {
            info!("设备 token 已轮换: {}", device_id);
            Ok(token)
        }
        None => anyhow::bail!("轮换失败：device_id {} 不存在", device_id),
    }
}
