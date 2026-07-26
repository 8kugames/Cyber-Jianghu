//! HTTP handlers: manual trigger, run catalog, download/delete, checkpoint debug.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::io::ReaderStream;

use crate::training_export::checkpoint::Checkpoint;
use crate::training_export::config::TrainingExportConfig;
use crate::training_export::{
    ExportRunRequest, RunMetadata, RunStatus, TriggerSource, validate_run_id,
};

const DEFAULT_LIST_LIMIT: usize = 100;
const MAX_LIST_LIMIT: usize = 1_000;

type ApiError = (StatusCode, Json<serde_json::Value>);

/// AppState 持有的训练导出配置与 manual queue sender。
#[derive(Clone, Debug)]
pub struct TrainingExportHandle {
    pub config: TrainingExportConfig,
    /// disabled 时为 None；handler 不得创建第二个执行路径。
    pub manual_tx: Option<mpsc::Sender<ExportRunRequest>>,
}

impl TrainingExportHandle {
    /// 构造一个 disabled handle：用于测试、文档示例与未来 read-only 部署。
    pub fn disabled(config: TrainingExportConfig) -> Self {
        Self {
            config,
            manual_tx: None,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct ExportRequest {
    pub agent_id: Option<uuid::Uuid>,
    #[serde(default)]
    pub force_full: bool,
}

#[derive(Debug, Serialize)]
pub struct ExportResponse {
    pub run_id: String,
    pub status: RunStatus,
    pub triggered_by: TriggerSource,
    pub started_at: i64,
}

pub async fn trigger_export(
    State(state): State<Arc<crate::state::AppState>>,
    Json(request): Json<ExportRequest>,
) -> Result<(StatusCode, Json<ExportResponse>), ApiError> {
    let handle = &state.training_export;
    if !handle.config.enabled {
        return Err(api_error(
            StatusCode::CONFLICT,
            "training_export_disabled",
            "训练导出未启用",
        ));
    }
    let sender = handle.manual_tx.as_ref().ok_or_else(|| {
        api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "training_export_unavailable",
            "训练导出 scheduler 不可用",
        )
    })?;

    let run_id = ulid::Ulid::new().to_string();
    let started_at = chrono::Utc::now().timestamp_millis();
    let run_request = build_manual_request(run_id.clone(), request);
    let enqueue_timeout = Duration::from_secs(handle.config.scheduler.manual_enqueue_timeout_secs);

    match tokio::time::timeout(enqueue_timeout, sender.send(run_request)).await {
        Ok(Ok(())) => {
            tracing::info!(run_id = %run_id, "手动训练导出请求已进入 scheduler queue");
            Ok((
                StatusCode::ACCEPTED,
                Json(ExportResponse {
                    run_id,
                    status: RunStatus::Pending,
                    triggered_by: TriggerSource::Manual,
                    started_at,
                }),
            ))
        }
        Ok(Err(_)) => Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "training_export_unavailable",
            "训练导出 scheduler queue 已关闭",
        )),
        Err(_) => Err(api_error(
            StatusCode::REQUEST_TIMEOUT,
            "training_export_queue_timeout",
            "等待训练导出 scheduler queue 超时",
        )),
    }
}

fn build_manual_request(run_id: String, request: ExportRequest) -> ExportRunRequest {
    ExportRunRequest {
        run_id,
        triggered_by: TriggerSource::Manual,
        agent_id_filter: request.agent_id,
        force_full: request.force_full,
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct ListExportsQuery {
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct ListExportsResponse {
    pub runs: Vec<RunMetadata>,
    pub total: usize,
    pub next_cursor: Option<String>,
}

pub async fn list_exports(
    State(state): State<Arc<crate::state::AppState>>,
    Query(query): Query<ListExportsQuery>,
) -> Result<Json<ListExportsResponse>, ApiError> {
    if let Some(cursor) = query.cursor.as_deref() {
        validate_external_run_id(cursor)?;
    }
    let limit = validate_list_limit(query.limit)?;
    let output_dir = output_dir(&state.training_export.config);
    let mut runs = read_all_metadata(&output_dir).await?;
    runs.sort_by(|left, right| right.run_id.cmp(&left.run_id));
    let total = runs.len();

    if let Some(cursor) = query.cursor.as_deref() {
        runs.retain(|metadata| metadata.run_id.as_str() < cursor);
    }

    let has_more = runs.len() > limit;
    runs.truncate(limit);
    let next_cursor = has_more
        .then(|| runs.last().map(|run| run.run_id.clone()))
        .flatten();

    Ok(Json(ListExportsResponse {
        runs,
        total,
        next_cursor,
    }))
}

pub async fn get_export(
    State(state): State<Arc<crate::state::AppState>>,
    Path(run_id): Path<String>,
) -> Result<Json<RunMetadata>, ApiError> {
    let meta_path = artifact_path(&state.training_export.config, &run_id, "meta.json")?;
    let content = match tokio::fs::read_to_string(&meta_path).await {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(run_not_found(&run_id));
        }
        Err(error) => return Err(io_error("读取训练导出元数据失败", error)),
    };
    let metadata = serde_json::from_str::<RunMetadata>(&content).map_err(|error| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "metadata_parse_failed",
            &format!("解析训练导出元数据失败: {error}"),
        )
    })?;
    if metadata.run_id != run_id {
        return Err(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "metadata_identity_invalid",
            "训练导出元数据 run_id 与请求路径不一致",
        ));
    }
    Ok(Json(metadata))
}

pub async fn download_export(
    State(state): State<Arc<crate::state::AppState>>,
    Path(run_id): Path<String>,
) -> Result<Response, ApiError> {
    let file_path = artifact_path(&state.training_export.config, &run_id, "jsonl")?;
    let file = match tokio::fs::File::open(&file_path).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(run_not_found(&run_id));
        }
        Err(error) => return Err(io_error("打开训练导出产物失败", error)),
    };

    let body = axum::body::Body::from_stream(ReaderStream::new(file));
    let mut response = body.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-jsonlines"),
    );
    let content_disposition = HeaderValue::from_str(&format!(
        "attachment; filename=\"run={run_id}.jsonl\""
    ))
    .map_err(|error| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "invalid_response_header",
            &format!("构造下载响应头失败: {error}"),
        )
    })?;
    response
        .headers_mut()
        .insert(header::CONTENT_DISPOSITION, content_disposition);
    Ok(response)
}

#[derive(Debug, Serialize)]
pub struct DeleteResponse {
    pub run_id: String,
    pub deleted: bool,
}

pub async fn delete_export(
    State(state): State<Arc<crate::state::AppState>>,
    Path(run_id): Path<String>,
) -> Result<Json<DeleteResponse>, ApiError> {
    let jsonl = artifact_path(&state.training_export.config, &run_id, "jsonl")?;
    let metadata = artifact_path(&state.training_export.config, &run_id, "meta.json")?;
    let mut deleted = false;

    for path in [jsonl, metadata] {
        match tokio::fs::remove_file(&path).await {
            Ok(()) => deleted = true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("删除训练导出文件失败", error)),
        }
    }

    if !deleted {
        return Err(run_not_found(&run_id));
    }
    Ok(Json(DeleteResponse { run_id, deleted }))
}

pub async fn get_checkpoint(
    State(state): State<Arc<crate::state::AppState>>,
) -> Result<Json<Checkpoint>, ApiError> {
    let path =
        crate::paths::get_data_dir().join(&state.training_export.config.paths.checkpoint_filename);
    let checkpoint = Checkpoint::load(&path)
        .await
        .map_err(|error| io_error("读取训练导出 checkpoint 失败", error))?;
    Ok(Json(checkpoint))
}

async fn read_all_metadata(output_dir: &std::path::Path) -> Result<Vec<RunMetadata>, ApiError> {
    let mut entries = match tokio::fs::read_dir(output_dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error("读取训练导出目录失败", error)),
    };
    let mut runs = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| io_error("遍历训练导出目录失败", error))?
    {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with("run=") || !name.ends_with(".meta.json") {
            continue;
        }
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|error| io_error("读取训练导出元数据失败", error))?;
        let metadata = serde_json::from_str::<RunMetadata>(&content).map_err(|error| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "metadata_parse_failed",
                &format!("解析训练导出元数据失败 {}: {error}", path.display()),
            )
        })?;
        if validate_run_id(&metadata.run_id).is_err()
            || name != format!("run={}.meta.json", metadata.run_id)
        {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "metadata_identity_invalid",
                &format!("训练导出元数据身份与文件名不一致: {}", path.display()),
            ));
        }
        runs.push(metadata);
    }
    Ok(runs)
}

fn validate_list_limit(limit: Option<usize>) -> Result<usize, ApiError> {
    let limit = limit.unwrap_or(DEFAULT_LIST_LIMIT);
    if !(1..=MAX_LIST_LIMIT).contains(&limit) {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "invalid_limit",
            &format!("limit 必须在 1..={MAX_LIST_LIMIT} 之间"),
        ));
    }
    Ok(limit)
}

fn validate_external_run_id(run_id: &str) -> Result<(), ApiError> {
    validate_run_id(run_id).map(|_| ()).map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            "invalid_run_id",
            "run_id 必须是合法 ULID",
        )
    })
}

fn artifact_path(
    config: &TrainingExportConfig,
    run_id: &str,
    suffix: &str,
) -> Result<PathBuf, ApiError> {
    validate_external_run_id(run_id)?;
    Ok(output_dir(config).join(format!("run={run_id}.{suffix}")))
}

fn output_dir(config: &TrainingExportConfig) -> PathBuf {
    crate::paths::get_data_dir().join(&config.paths.output_subdir)
}

fn run_not_found(run_id: &str) -> ApiError {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "run_not_found", "run_id": run_id})),
    )
}

fn io_error(context: &str, error: impl std::fmt::Display) -> ApiError {
    api_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "io_error",
        &format!("{context}: {error}"),
    )
}

fn api_error(status: StatusCode, code: &str, message: &str) -> ApiError {
    (
        status,
        Json(serde_json::json!({"error": code, "message": message})),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        ExportRequest, build_manual_request, validate_external_run_id, validate_list_limit,
    };
    use crate::training_export::TriggerSource;

    #[test]
    fn manual_request_preserves_filter_and_force_full() {
        let agent_id = uuid::Uuid::new_v4();
        let request = build_manual_request(
            "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            ExportRequest {
                agent_id: Some(agent_id),
                force_full: true,
            },
        );
        assert_eq!(request.triggered_by, TriggerSource::Manual);
        assert_eq!(request.agent_id_filter, Some(agent_id));
        assert!(request.force_full);
    }

    #[test]
    fn invalid_run_ids_are_rejected_before_path_use() {
        assert!(validate_external_run_id("../secret").is_err());
        assert!(validate_external_run_id("01ARZ3NDEKTSV4RRFFQ69G5FAV/path").is_err());
        assert!(validate_external_run_id("not-a-ulid").is_err());
    }

    #[test]
    fn list_limit_is_bounded() {
        assert_eq!(validate_list_limit(None).unwrap(), 100);
        assert!(validate_list_limit(Some(0)).is_err());
        assert!(validate_list_limit(Some(1_001)).is_err());
    }

    #[test]
    fn api_error_message_carries_error_code() {
        let (status, body) = super::api_error(
            axum::http::StatusCode::CONFLICT,
            "training_export_disabled",
            "训练导出未启用",
        );
        assert_eq!(status, axum::http::StatusCode::CONFLICT);
        let value = body.0;
        assert_eq!(value["error"], "training_export_disabled");
        assert_eq!(value["message"], "训练导出未启用");
    }

    #[test]
    fn artifact_path_uses_validated_run_id() {
        use crate::training_export::config::TrainingExportConfig;
        let config = TrainingExportConfig::default();
        let result = super::artifact_path(&config, "../escape", "jsonl");
        assert!(result.is_err(), "artifact_path must reject unsafe run_id");
    }
}
