// ============================================================================
// 自更新 HTTP Handlers（GitHub Release）
// ============================================================================
//
// 仅做触发与状态透出；更新决策/下载/安装逻辑全部收敛在 infra/updater.rs
// （agent-self-update seam 的 single_writer）。
//
// 认证：非公开路径，走 agent HTTP API 的 Bearer token（与 /api/v1/config 同级）。

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde_json::json;

use super::HttpApiState;
use crate::infra::updater::{ApplyOutcome, Updater};

/// apply 响应发送后、重启前的延迟（给客户端留出接收窗口）
const RESTART_DELAY_MS: u64 = 800;

/// GET /api/v1/update/status — 当前版本 / 最新 release / 上次检查结果
pub(crate) async fn get_update_status_handler(
    State(state): State<HttpApiState>,
) -> impl IntoResponse {
    Json(state.updater.status().await).into_response()
}

/// POST /api/v1/update/check — 立即向 GitHub 检查最新 release
pub(crate) async fn post_update_check_handler(
    State(state): State<HttpApiState>,
) -> axum::response::Response {
    match state.updater.check().await {
        Ok(r) => Json(json!({
            "release_tag": r.release_tag,
            "asset_name": r.asset_name,
            "asset_digest": r.asset_digest,
            "current_digest": r.current_digest,
            "update_available": r.update_available,
        }))
        .into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": format!("{e:#}") })),
        )
            .into_response(),
    }
}

/// POST /api/v1/update/apply — 下载安装最新版本并重启进程
///
/// 先发送响应再延迟重启（给客户端留出接收窗口）；unix 上 restart 通过
/// execve 自替换，响应发送失败也不阻断更新流程。
pub(crate) async fn post_update_apply_handler(
    State(state): State<HttpApiState>,
) -> axum::response::Response {
    match state.updater.apply().await {
        Ok(ApplyOutcome::UpToDate) => Json(json!({
            "applied": false,
            "reason": "up_to_date",
        }))
        .into_response(),
        Ok(ApplyOutcome::Installed { tag }) => {
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(RESTART_DELAY_MS)).await;
                if let Err(e) = Updater::restart_self() {
                    tracing::error!("更新已安装但重启失败，等待下次手动重启: {e:#}");
                }
            });
            Json(json!({
                "applied": true,
                "tag": tag,
                "restarting": true,
            }))
            .into_response()
        }
        Err(e) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": format!("{e:#}") })),
        )
            .into_response(),
    }
}
