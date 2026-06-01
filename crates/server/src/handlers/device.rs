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

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use std::sync::Arc;
use tracing::{info, warn};

use crate::db;
use crate::models::{
    DeviceRegisterErrorResponse, DeviceRegisterResponse, DeviceVerifyErrorResponse,
    DeviceVerifyRequest, DeviceVerifyResponse,
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
                message: "设备不存在".to_string(),
                device_id: payload.device_id,
            }),
        ));
    }

    // 设备存在 → 仅取 token（SELECT only，根除 TOCTOU 竞态）
    // 关键：绝不可调 connect_device（upsert 语义），否则 admin DELETE 后
    // verify 会"复活"刚被删的 device，破坏 server 权威性。
    let auth_token = db::get_device_token(&state.db_pool, payload.device_id)
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
        })?
        .ok_or_else(|| {
            // verify_device_strict 已确认存在，get_device_token 却又找不到 —
            // 说明 verify 与 get_device_token 之间发生了 DELETE（并发窗口）。
            // 此时必须 fail-fast 报错，**不可**回落 upsert 创建。
            warn!(
                "设备 {} 在 verify 与取 token 之间消失（并发 DELETE），fail-fast",
                payload.device_id
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(DeviceVerifyErrorResponse {
                    error: "internal_error",
                    message: "设备状态在请求中途发生变化".to_string(),
                    device_id: payload.device_id,
                }),
            )
        })?;

    Ok(Json(DeviceVerifyResponse {
        device_id: payload.device_id,
        auth_token,
        message: format!("设备 {} 校验通过", payload.device_id),
    }))
}

/// 设备显式注册
///
/// POST /api/v1/device/register
///
/// Agent 申报注册新设备。**不接受** device_id 入参，server 端生成 UUID v4。
/// 这是消除"client 携带任意 UUID 撞库"的协议层保证。
///
/// 成功 → 201 Created（资源创建）
/// 失败 → 5xx + 结构化 JSON body（与 verify 错误响应对称）
pub async fn device_register(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<DeviceRegisterErrorResponse>)> {
    let result = db::register_device(&state.db_pool).await.map_err(|e| {
        warn!("设备显式注册失败: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(DeviceRegisterErrorResponse {
                error: "internal_error",
                message: "服务器内部错误".to_string(),
            }),
        )
    })?;

    info!("设备显式注册成功: {}", result.device_id);

    Ok((
        StatusCode::CREATED,
        Json(DeviceRegisterResponse {
            device_id: result.device_id,
            auth_token: result.auth_token,
            message: format!("设备 {} 注册成功", result.device_id),
        }),
    ))
}
