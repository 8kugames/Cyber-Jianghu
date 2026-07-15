// ============================================================================
// 对话聚合端点 (C4)
// ============================================================================
//
// GET /api/dashboard/dialogues
//
// 从 agent_action_logs 聚合 speak 动作（action_type = '说话'），返回最近的对话流。
// action_data->>'channel' 区分 public/private/broadcast，content 取 action_data->>'content'。
// 前端可用于展示世界聊天流、私语索引。
//
// 可选查询参数：
//   ?limit=N   最多返回条数（默认 50，上限 200）
//   ?tick_from=最小 tick_id（默认不过滤）
// ============================================================================

use axum::{
    Json,
    extract::{Query, State},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

use crate::state::AppState;

// ============================================================================
// 查询参数
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct DialogueQuery {
    /// 最多返回条数（默认 50，上限 200）
    #[serde(default = "default_limit")]
    pub limit: i64,
    /// 最小 tick_id（默认不过滤）
    pub tick_from: Option<i64>,
}

fn default_limit() -> i64 {
    50
}

// ============================================================================
// 响应结构
// ============================================================================

/// 对话聚合响应
#[derive(Debug, Serialize)]
pub struct DialoguesResponse {
    /// 返回的对话条数
    pub count: usize,
    /// 对话流（按时间正序，最早的在前 —— 便于前端直接拼接展示）
    pub dialogues: Vec<DialogueEntry>,
}

/// 单条对话
#[derive(Debug, Serialize)]
pub struct DialogueEntry {
    pub id: i64,
    pub tick_id: i64,
    pub agent_id: Uuid,
    /// 说话者名称（agents.name，可能为空 → "unknown"）
    pub agent_name: String,
    /// 发话时所在节点（最新 agent_states.node_id 近似；NULL → "unknown"）
    pub location: String,
    /// 频道：public / private / broadcast
    pub channel: String,
    /// 目标 agent_id（仅 private 频道有）
    pub target_agent_id: Option<String>,
    /// 对话内容
    pub content: String,
    /// 动作执行结果
    pub result: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

// ============================================================================
// Handler
// ============================================================================

/// GET /api/dashboard/dialogues
///
/// 聚合最近的 speak（说话）动作。action_data 内 channel 区分公开/私语/广播。
pub async fn get_dialogues(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DialogueQuery>,
) -> Json<DialoguesResponse> {
    // 限制 limit 上限，防滥用
    let limit = query.limit.clamp(1, 200);

    // 查 speak 动作；action_data->>'channel'/'content'/'target_agent_id' 取字段
    // 按 created_at DESC 取最近 N 条，再在 Rust 层反转为正序输出
    let sql = if query.tick_from.is_some() {
        r#"
        SELECT
            l.id,
            l.tick_id,
            l.agent_id,
            a.name as agent_name,
            COALESCE(s.node_id, 'unknown') as location,
            COALESCE(l.action_data->>'channel', 'public') as channel,
            l.action_data->>'target_agent_id' as target_agent_id,
            COALESCE(l.action_data->>'content', '') as content,
            l.result,
            l.created_at
        FROM agent_action_logs l
        LEFT JOIN agents a ON a.agent_id = l.agent_id
        LEFT JOIN LATERAL (
            SELECT node_id FROM agent_states
            WHERE agent_states.agent_id = l.agent_id
            ORDER BY tick_id DESC LIMIT 1
        ) s ON true
        WHERE l.action_type = '说话'
          AND l.tick_id >= $1
        ORDER BY l.created_at DESC
        LIMIT $2
        "#
    } else {
        r#"
        SELECT
            l.id,
            l.tick_id,
            l.agent_id,
            a.name as agent_name,
            COALESCE(s.node_id, 'unknown') as location,
            COALESCE(l.action_data->>'channel', 'public') as channel,
            l.action_data->>'target_agent_id' as target_agent_id,
            COALESCE(l.action_data->>'content', '') as content,
            l.result,
            l.created_at
        FROM agent_action_logs l
        LEFT JOIN agents a ON a.agent_id = l.agent_id
        LEFT JOIN LATERAL (
            SELECT node_id FROM agent_states
            WHERE agent_states.agent_id = l.agent_id
            ORDER BY tick_id DESC LIMIT 1
        ) s ON true
        WHERE l.action_type = '说话'
        ORDER BY l.created_at DESC
        LIMIT $1
        "#
    };

    let rows = if let Some(tick_from) = query.tick_from {
        sqlx::query(sql)
            .bind(tick_from)
            .bind(limit)
            .fetch_all(&state.db_pool)
            .await
    } else {
        sqlx::query(sql).bind(limit).fetch_all(&state.db_pool).await
    };

    let rows = match rows {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("查询 dialogues 失败: {}", e);
            return Json(DialoguesResponse {
                count: 0,
                dialogues: Vec::new(),
            });
        }
    };

    let mut dialogues: Vec<DialogueEntry> = rows
        .into_iter()
        .map(|row| DialogueEntry {
            id: row.get("id"),
            tick_id: row.get("tick_id"),
            agent_id: row.get("agent_id"),
            agent_name: row
                .get::<Option<String>, _>("agent_name")
                .unwrap_or_else(|| "unknown".to_string()),
            location: row.get("location"),
            channel: row.get("channel"),
            target_agent_id: row.get("target_agent_id"),
            content: row.get("content"),
            result: row.get("result"),
            created_at: row.get("created_at"),
        })
        .collect();

    // 反转为正序（最早在前），便于前端拼接
    dialogues.reverse();

    let count = dialogues.len();
    Json(DialoguesResponse {
        count,
        dialogues,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limit_is_50() {
        assert_eq!(default_limit(), 50);
    }

    #[test]
    fn dialogues_response_is_serialize() {
        fn assert_serialize<T: serde::Serialize>() {}
        assert_serialize::<DialoguesResponse>();
        assert_serialize::<DialogueEntry>();
    }
}
