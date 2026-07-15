// ============================================================================
// Agent 关系图谱 API Handler
// ============================================================================
//
// GET /api/dashboard/agent-relationships              - 全局所有关系
// GET /api/dashboard/agent-relationships/{agent_id}   - 单个 Agent 的所有关系
//
// 数据来自 agent_relationships + agent_relationship_key_events（C1 全量快照同步）。
// 返回完整的 protocol RelationshipMemory（含 key_events），前端可一次取齐。
// ============================================================================

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use uuid::Uuid;

use crate::db;
use crate::state::AppState;
use cyber_jianghu_protocol::types::{RelationshipKeyEvent, RelationshipMemory};

// ============================================================================
// 响应类型
// ============================================================================

/// 单条关系（带 source_agent_id，便于前端按节点构建有向图）
#[derive(Debug, serde::Serialize)]
pub struct RelationshipItem {
    pub source_agent_id: Uuid,
    #[serde(flatten)]
    pub relationship: RelationshipMemory,
}

/// 列表响应
#[derive(Debug, serde::Serialize)]
pub struct ListResponse {
    pub relationships: Vec<RelationshipItem>,
    pub total: i64,
}

// ============================================================================
// 端点
// ============================================================================

/// 路径参数
#[derive(Debug, Deserialize)]
pub struct AgentIdPath {
    pub agent_id: String,
}

/// 获取所有 Agent 的关系图谱
/// GET /api/dashboard/agent-relationships
pub async fn get_all_relationships(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ListResponse>, axum::http::StatusCode> {
    let rows = db::get_all_relationships(&state.db_pool).await.map_err(|e| {
        tracing::error!("查询 agent_relationships 全表失败: {}", e);
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let relationships: Vec<RelationshipItem> = rows
        .into_iter()
        .map(|(source_agent_id, relationship)| RelationshipItem {
            source_agent_id,
            relationship,
        })
        .collect();
    let total = relationships.len() as i64;

    Ok(Json(ListResponse {
        relationships,
        total,
    }))
}

/// 获取指定 Agent 的所有关系（作为 source）
/// GET /api/dashboard/agent-relationships/{agent_id}
pub async fn get_relationships_by_agent(
    State(state): State<Arc<AppState>>,
    Path(params): Path<AgentIdPath>,
) -> Result<Json<ListResponse>, axum::http::StatusCode> {
    let agent_id =
        Uuid::parse_str(&params.agent_id).map_err(|_| axum::http::StatusCode::BAD_REQUEST)?;

    let rows = db::get_relationships_by_agent(&state.db_pool, agent_id)
        .await
        .map_err(|e| {
            tracing::error!("查询 Agent {} relationships 失败: {}", agent_id, e);
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let relationships: Vec<RelationshipItem> = rows
        .into_iter()
        .map(|relationship| RelationshipItem {
            source_agent_id: agent_id,
            relationship,
        })
        .collect();
    let total = relationships.len() as i64;

    Ok(Json(ListResponse {
        relationships,
        total,
    }))
}

// 静默使用 RelationshipKeyEvent（确保类型导入在序列化路径上被识别）
#[allow(dead_code)]
type _KeyEventUsed = RelationshipKeyEvent;
