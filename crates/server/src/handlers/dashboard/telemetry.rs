use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::state::AppState;
use crate::telemetry::storage;

fn default_limit() -> i64 {
    100
}

/// 查询参数
#[derive(Debug, Deserialize)]
pub struct TelemetryListQuery {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

/// GET /api/dashboard/telemetry — 列出所有可用的聚合名称
pub async fn list_telemetry_aggregations(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<String>>, StatusCode> {
    match storage::list_aggregation_names(&state.db_pool).await {
        Ok(names) => Ok(Json(names)),
        Err(e) => {
            tracing::warn!("查询遥测聚合列表失败: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// GET /api/dashboard/telemetry/{aggregation_name} — 查询指定聚合的数据
pub async fn get_telemetry_aggregation(
    State(state): State<Arc<AppState>>,
    Path(aggregation_name): Path<String>,
    Query(query): Query<TelemetryListQuery>,
) -> Result<Json<Vec<crate::telemetry::TelemetryRow>>, StatusCode> {
    // 校验聚合名称合法性（只允许字母数字下划线，防止 SQL 注入——虽然已用参数化查询）
    if aggregation_name
        .chars()
        .any(|c| !c.is_alphanumeric() && c != '_')
    {
        return Err(StatusCode::BAD_REQUEST);
    }

    match storage::query_aggregations(&state.db_pool, &aggregation_name, query.limit, query.offset)
        .await
    {
        Ok(rows) => Ok(Json(rows)),
        Err(e) => {
            tracing::warn!("查询遥测聚合 {} 失败: {}", aggregation_name, e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}
