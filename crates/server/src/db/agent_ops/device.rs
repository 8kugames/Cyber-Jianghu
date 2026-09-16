// ============================================================================
// 设备连接与验证（connect / verify / register / last_seen）
// ============================================================================

use super::*;

// ============================================================================
// 设备连接
// ============================================================================

/// 设备连接结果
#[derive(Debug)]
pub struct DeviceConnectResult {
    /// 设备 ID
    pub device_id: Uuid,
    /// 认证令牌
    pub auth_token: String,
    /// 是否为新设备
    pub is_new: bool,
}

/// 注册或获取设备
///
/// - 如果设备不存在，创建新设备记录并生成 auth_token
/// - 如果设备已存在，返回现有的 auth_token
///
/// # 参数
/// - pool: 数据库连接池
/// - device_id: 客户端生成的设备 UUID
///
/// # 返回
/// - Ok(DeviceConnectResult): 连接结果
/// - Err: 数据库操作失败
pub async fn connect_device(pool: &PgPool, device_id: Uuid) -> Result<DeviceConnectResult> {
    debug!("设备连接: {}", device_id);

    // 先尝试获取现有设备
    let existing: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT auth_token FROM devices WHERE device_id = $1
        "#,
    )
    .bind(device_id)
    .fetch_optional(pool)
    .await
    .context("查询设备失败")?;

    if let Some((auth_token,)) = existing {
        debug!("设备已存在: {}", device_id);
        return Ok(DeviceConnectResult {
            device_id,
            auth_token,
            is_new: false,
        });
    }

    // 创建新设备
    let auth_token = generate_secure_token();

    sqlx::query(
        r#"
        INSERT INTO devices (device_id, auth_token)
        VALUES ($1, $2)
        ON CONFLICT (device_id) DO UPDATE SET last_seen = CURRENT_TIMESTAMP
        "#,
    )
    .bind(device_id)
    .bind(&auth_token)
    .execute(pool)
    .await
    .context("创建设备记录失败")?;

    tracing::info!("新设备注册成功: {}", device_id);

    Ok(DeviceConnectResult {
        device_id,
        auth_token,
        is_new: true,
    })
}

/// 仅查询设备当前 auth_token（SELECT only，无副作用）
///
/// 与 `connect_device` 的根本区别：
/// - 本函数**永远不修改数据库**，调用方必须先通过 `verify_device_strict`
///   确认设备存在后才能调用，否则会得到 `Ok(None)`
/// - `connect_device` 在设备不存在时会自动 INSERT，是 upsert 语义
///
/// 用于 `device_verify` 端点的 200 路径。**绝不**用于任何需要"创建/复活"
/// 设备的场景——那是 `register_device` 的责任。
///
/// # 参数
/// - pool: 数据库连接池
/// - device_id: 设备 UUID
///
/// # 返回
/// - Ok(Some(token)): 设备存在，返回当前 auth_token
/// - Ok(None): 设备不存在
/// - Err: 数据库查询失败
pub async fn get_device_token(pool: &PgPool, device_id: Uuid) -> Result<Option<String>> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT auth_token FROM devices WHERE device_id = $1")
            .bind(device_id)
            .fetch_optional(pool)
            .await
            .context("查询设备 token 失败")?;
    Ok(row.map(|(t,)| t))
}

/// 严格校验设备是否存在（仅查询，不创建）
///
/// 与 `connect_device` 的根本区别：本函数**不会**因为设备不存在而自动创建。
/// 用于 agent 启动时验证 device.yaml 中的 device_id 仍被 server 认可。
///
/// # 参数
/// - pool: 数据库连接池
/// - device_id: 设备 UUID
///
/// # 返回
/// - Ok(true): 设备存在
/// - Ok(false): 设备不存在
/// - Err: 数据库错误
pub async fn verify_device_strict(pool: &PgPool, device_id: Uuid) -> Result<bool> {
    let row: Option<(Uuid,)> = sqlx::query_as("SELECT device_id FROM devices WHERE device_id = $1")
        .bind(device_id)
        .fetch_optional(pool)
        .await
        .context("严格校验设备失败")?;
    Ok(row.is_some())
}

/// 显式注册新设备（server 生成 device_id + auth_token）
///
/// 与 `connect_device` 的根本区别：调用者**不能**指定 device_id，必须由 server 生成。
/// 这样从协议层面消除"client 携带任意 UUID 撞库"的可能。
///
/// # 参数
/// - pool: 数据库连接池
///
/// # 返回
/// - Ok(DeviceConnectResult): 包含新 device_id + auth_token，is_new 恒为 true
/// - Err: 数据库错误
pub async fn register_device(pool: &PgPool) -> Result<DeviceConnectResult> {
    let device_id = Uuid::new_v4();
    let auth_token = generate_secure_token();

    sqlx::query(
        r#"
        INSERT INTO devices (device_id, auth_token)
        VALUES ($1, $2)
        "#,
    )
    .bind(device_id)
    .bind(&auth_token)
    .execute(pool)
    .await
    .context("显式注册新设备失败")?;

    info!("新设备显式注册成功: {}", device_id);

    Ok(DeviceConnectResult {
        device_id,
        auth_token,
        is_new: true,
    })
}

/// 验证设备认证令牌
///
/// # 参数
/// - pool: 数据库连接池
/// - device_id: 设备 UUID
/// - auth_token: 认证令牌
///
/// # 返回
/// - Ok(true): 验证通过
/// - Ok(false): 验证失败
/// - Err: 数据库错误
pub async fn verify_device_token(pool: &PgPool, device_id: Uuid, auth_token: &str) -> Result<bool> {
    let result: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM devices WHERE device_id = $1 AND auth_token = $2
        "#,
    )
    .bind(device_id)
    .bind(auth_token)
    .fetch_optional(pool)
    .await
    .context("验证设备令牌失败")?;

    Ok(result.is_some())
}

/// 仅通过 auth_token 查找设备（proposal 提交端点使用，无需 device_id）
pub async fn find_device_by_auth_token(pool: &PgPool, auth_token: &str) -> Result<Option<Uuid>> {
    let result: Option<(Uuid,)> =
        sqlx::query_as(r#"SELECT device_id FROM devices WHERE auth_token = $1"#)
            .bind(auth_token)
            .fetch_optional(pool)
            .await
            .context("按 auth_token 查找设备失败")?;

    Ok(result.map(|(id,)| id))
}

/// 更新设备最后在线时间
pub async fn update_device_last_seen(pool: &PgPool, device_id: Uuid) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE devices SET last_seen = CURRENT_TIMESTAMP WHERE device_id = $1
        "#,
    )
    .bind(device_id)
    .execute(pool)
    .await
    .context("更新设备在线时间失败")?;

    Ok(())
}
