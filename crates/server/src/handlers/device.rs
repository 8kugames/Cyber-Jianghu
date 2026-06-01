// ============================================================================
// 设备身份生命周期 v2 — 严格校验 + 显式注册
// ============================================================================
//
// 与 `agent.rs::agent_connect` 的根本区别：
// - agent_connect 是 upsert 语义（撞库空就建）— **保留以兼容旧 agent**
// - 本文件的两个端点是严格语义：
//   * /device/verify   — 仅查询，设备不存在返回 404
//   * /device/register — server 端生成 device_id，**不允许** client 传入
//
// 配合 agent 端 `ensure_device` 的 fail-fast 验证，形成完整生命周期。
// ============================================================================

use axum::{Json, extract::State, http::StatusCode};
use std::sync::Arc;
use tracing::{info, warn};

use crate::db;
use crate::models::{
    DeviceRegisterResponse, DeviceVerifyErrorResponse, DeviceVerifyRequest, DeviceVerifyResponse,
};
use crate::state::AppState;

/// 设备严格校验
///
/// POST /api/v1/device/verify
///
/// Agent 启动时携带本地 device.yaml 中的 device_id 向 server 验证仍被认可。
/// - 设备存在 → 200 + 当前 auth_token（可能与本地不一致，以 server 为准）
/// - 设备不存在 → 404 + `{error: "device_not_found", ...}`，要求 agent 走 /device/register
pub async fn device_verify(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<DeviceVerifyRequest>,
) -> Result<Json<DeviceVerifyResponse>, (StatusCode, Json<DeviceVerifyErrorResponse>)> {
    info!("设备严格校验请求: {}", payload.device_id);

    let exists = db::verify_device_strict(&state.db_pool, payload.device_id)
        .await
        .map_err(|e| {
            warn!("设备校验数据库错误: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(DeviceVerifyErrorResponse {
                    error: "internal_error",
                    message: "服务器内部错误".to_string(),
                    device_id: payload.device_id,
                }),
            )
        })?;

    if !exists {
        warn!("设备 {} 不存在，要求 agent 重新注册", payload.device_id);
        return Err((
            StatusCode::NOT_FOUND,
            Json(DeviceVerifyErrorResponse {
                error: "device_not_found",
                message: "设备不存在，请调用 /api/v1/device/register 重新注册".to_string(),
                device_id: payload.device_id,
            }),
        ));
    }

    // 设备存在 → 取出当前 token（用 connect_device 的只读路径）
    let result = db::connect_device(&state.db_pool, payload.device_id)
        .await
        .map_err(|e| {
            warn!("设备 {} 取 token 失败: {}", payload.device_id, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(DeviceVerifyErrorResponse {
                    error: "internal_error",
                    message: "服务器内部错误".to_string(),
                    device_id: payload.device_id,
                }),
            )
        })?;

    Ok(Json(DeviceVerifyResponse {
        device_id: result.device_id,
        auth_token: result.auth_token,
        message: format!("设备 {} 校验通过", result.device_id),
    }))
}

/// 设备显式注册
///
/// POST /api/v1/device/register
///
/// Agent 申报注册新设备。**不接受** device_id 入参，server 端生成 UUID v4。
/// 这是消除"client 携带任意 UUID 撞库"的协议层保证。
pub async fn device_register(
    State(state): State<Arc<AppState>>,
) -> Result<Json<DeviceRegisterResponse>, StatusCode> {
    let result = db::register_device(&state.db_pool)
        .await
        .map_err(|e| {
            warn!("设备显式注册失败: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    info!("设备显式注册成功: {}", result.device_id);

    // 设备注册响应不下发 narrative_config — 那是 /connect 端点的责任
    // 这样保持 /device/register 是纯粹的"设备身份发放"

    Ok(Json(DeviceRegisterResponse {
        device_id: result.device_id,
        auth_token: result.auth_token,
        message: format!("设备 {} 注册成功", result.device_id),
    }))
}
