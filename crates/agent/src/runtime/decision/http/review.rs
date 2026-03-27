//! 审查系统 HTTP API
//!
//! 用于 Observer Agent 审查 Player Agent 意图的 API 端点。

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use chrono::{Duration, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::config::ReviewConfig;

// Import HttpApiState from parent module
use super::HttpApiState;

// Re-export protocol types
pub use cyber_jianghu_protocol::{
    PendingReview, PersonaSummary, ReviewDecision, ReviewError, ReviewErrorResponse, ReviewResult,
    ReviewStatus, ReviewSubmission,
};

// ============================================================================
// 审查状态存储
// ============================================================================

/// 待审查意图条目（内部存储用）
#[derive(Debug, Clone)]
pub struct PendingReviewEntry {
    /// 意图 ID
    pub intent_id: Uuid,
    /// 玩家 Agent ID
    pub agent_id: Uuid,
    /// 意图内容
    pub intent: cyber_jianghu_protocol::Intent,
    /// 人设摘要
    pub persona_summary: PersonaSummary,
    /// 世界上下文
    pub world_context: String,
    /// 创建时间
    pub created_at: chrono::DateTime<Utc>,
    /// 审查截止时间
    pub deadline: chrono::DateTime<Utc>,
}

/// 审查状态存储
///
/// 管理待审查意图和审查结果
#[derive(Debug, Default)]
pub struct ReviewStore {
    /// 待审查意图（intent_id -> entry）
    pending: RwLock<HashMap<Uuid, PendingReviewEntry>>,
    /// 审查结果（intent_id -> result）
    results: RwLock<HashMap<Uuid, ReviewResult>>,
    /// 审查配置
    config: ReviewConfig,
}

impl ReviewStore {
    /// 创建新的审查存储
    pub fn new(config: ReviewConfig) -> Self {
        Self {
            pending: RwLock::new(HashMap::new()),
            results: RwLock::new(HashMap::new()),
            config,
        }
    }

    /// 添加待审查意图
    pub async fn add_pending(
        &self,
        intent: cyber_jianghu_protocol::Intent,
        agent_id: Uuid,
        persona_summary: PersonaSummary,
        world_context: String,
    ) -> Uuid {
        let intent_id = Uuid::new_v4();
        let now = Utc::now();
        let deadline = now + Duration::seconds(self.config.timeout_seconds as i64);

        let entry = PendingReviewEntry {
            intent_id,
            agent_id,
            intent,
            persona_summary,
            world_context,
            created_at: now,
            deadline,
        };

        let mut pending = self.pending.write().await;
        pending.insert(intent_id, entry);

        debug!("[review] Added pending review: intent_id={}", intent_id);
        intent_id
    }

    /// 获取所有待审查意图
    pub async fn get_pending(&self) -> Vec<PendingReview> {
        let pending = self.pending.read().await;
        pending
            .values()
            .map(|e| PendingReview {
                intent_id: e.intent_id,
                agent_id: e.agent_id,
                intent: e.intent.clone(),
                persona_summary: e.persona_summary.clone(),
                world_context: e.world_context.clone(),
                created_at: e.created_at,
                deadline: e.deadline,
            })
            .collect()
    }

    /// 获取特定意图的审查状态
    pub async fn get_status(&self, intent_id: Uuid) -> Option<ReviewResult> {
        let results = self.results.read().await;
        results.get(&intent_id).cloned()
    }

    /// 检查意图是否待审查
    pub async fn is_pending(&self, intent_id: Uuid) -> bool {
        let pending = self.pending.read().await;
        pending.contains_key(&intent_id)
    }

    /// 获取待审查意图的 tick_id
    ///
    /// 用于在提交审查时更新 intent_history
    pub async fn get_tick_id(&self, intent_id: Uuid) -> Option<i64> {
        let pending = self.pending.read().await;
        pending.get(&intent_id).map(|e| e.intent.tick_id)
    }

    /// 提交审查结果
    pub async fn submit_review(
        &self,
        intent_id: Uuid,
        submission: ReviewSubmission,
    ) -> Result<ReviewResult, ReviewError> {
        // 检查是否已审查
        {
            let results = self.results.read().await;
            if results.contains_key(&intent_id) {
                return Err(ReviewError::AlreadyReviewed { intent_id });
            }
        }

        // 检查是否待审查
        {
            let pending = self.pending.read().await;
            if !pending.contains_key(&intent_id) {
                return Err(ReviewError::IntentNotFound { intent_id });
            }
        }

        // 构建结果
        let now = Utc::now();
        let status = match submission.result {
            ReviewDecision::Approved => ReviewStatus::Approved,
            ReviewDecision::Rejected => ReviewStatus::Rejected,
        };

        let result = ReviewResult {
            intent_id,
            status,
            decision: Some(submission.result),
            reason: Some(submission.reason),
            narrative: submission.narrative,
            reviewed_at: now,
        };

        // 存储结果并移除待审查
        {
            let mut results = self.results.write().await;
            results.insert(intent_id, result.clone());
        }
        {
            let mut pending = self.pending.write().await;
            pending.remove(&intent_id);
        }

        info!(
            "[review] Review submitted: intent_id={}, status={:?}",
            intent_id, result.status
        );
        Ok(result)
    }

    /// 处理超时的审查（自动通过）
    pub async fn process_timeouts(&self) -> Vec<ReviewResult> {
        let now = Utc::now();
        let mut expired = Vec::new();

        // 找出过期的待审查
        {
            let pending = self.pending.read().await;
            for (intent_id, entry) in pending.iter() {
                if now > entry.deadline {
                    expired.push(*intent_id);
                }
            }
        }

        // 处理过期
        let mut results = Vec::new();
        for intent_id in expired {
            let reviewed_at = Utc::now();
            let result = ReviewResult {
                intent_id,
                status: ReviewStatus::TimeoutApproved,
                decision: None,
                reason: Some("审查超时，自动通过".to_string()),
                narrative: None,
                reviewed_at,
            };

            {
                let mut results_map = self.results.write().await;
                results_map.insert(intent_id, result.clone());
            }
            {
                let mut pending = self.pending.write().await;
                pending.remove(&intent_id);
            }

            warn!(
                "[review] Review timeout, auto-approved: intent_id={}",
                intent_id
            );
            results.push(result);
        }

        results
    }

    /// 获取意图（用于审查后提交）
    pub async fn get_intent(&self, intent_id: Uuid) -> Option<cyber_jianghu_protocol::Intent> {
        // 先从待审查中查找
        {
            let pending = self.pending.read().await;
            if let Some(entry) = pending.get(&intent_id) {
                return Some(entry.intent.clone());
            }
        }
        None
    }
}

// ============================================================================
// HTTP 处理函数
// ============================================================================

/// 审查存储状态
pub type ReviewState = Arc<ReviewStore>;

/// GET /api/v1/review/pending - 获取待审查意图
pub async fn get_pending_reviews(
    State(api_state): State<HttpApiState>,
) -> Result<Json<Vec<PendingReview>>, (StatusCode, Json<ReviewErrorResponse>)> {
    let Some(review_store) = &api_state.review_store else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ReviewError::ReviewDisabled.to_response()),
        ));
    };

    let pending = review_store.get_pending().await;
    Ok(Json(pending))
}

/// POST /api/v1/review/{intent_id} - 提交审查结果
pub async fn submit_review(
    State(api_state): State<HttpApiState>,
    Path(intent_id): Path<Uuid>,
    Json(submission): Json<ReviewSubmission>,
) -> Result<Json<ReviewResult>, (StatusCode, Json<ReviewErrorResponse>)> {
    let Some(review_store) = &api_state.review_store else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ReviewError::ReviewDisabled.to_response()),
        ));
    };

    // 获取 pending entry 以获取 tick_id（用于更新 intent_history）
    let tick_id = review_store.get_tick_id(intent_id).await;

    match review_store
        .submit_review(intent_id, submission.clone())
        .await
    {
        Ok(result) => {
            // 更新 intent_history 中的 observer_thought
            if let (Some(tick_id), Some(history)) = (tick_id, &api_state.intent_history) {
                // 使用 reason 作为 observer_thought（审查原因即 Observer 的思维链）
                history
                    .update_observer_thought(tick_id, submission.reason.clone())
                    .await;
                info!(
                    "[review] Updated observer thought for tick {} in intent_history",
                    tick_id
                );
            }
            Ok(Json(result))
        }
        Err(e) => {
            let status = match &e {
                ReviewError::IntentNotFound { .. } => StatusCode::NOT_FOUND,
                ReviewError::AlreadyReviewed { .. } => StatusCode::CONFLICT,
                ReviewError::Unauthorized => StatusCode::UNAUTHORIZED,
                ReviewError::ReviewDisabled => StatusCode::SERVICE_UNAVAILABLE,
            };
            Err((status, Json(e.to_response())))
        }
    }
}

/// GET /api/v1/review/{intent_id}/status - 获取审查状态
pub async fn get_review_status(
    State(api_state): State<HttpApiState>,
    Path(intent_id): Path<Uuid>,
) -> Result<Json<ReviewResult>, (StatusCode, Json<ReviewErrorResponse>)> {
    let Some(review_store) = &api_state.review_store else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ReviewError::ReviewDisabled.to_response()),
        ));
    };

    match review_store.get_status(intent_id).await {
        Some(result) => Ok(Json(result)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(ReviewError::IntentNotFound { intent_id }.to_response()),
        )),
    }
}

// ============================================================================
// 超时处理后台任务
// ============================================================================

/// 启动审查超时处理后台任务
///
/// 定期检查待审查意图，如果超时则自动通过
///
/// # 参数
///
/// - `review_store`: 审查存储
/// - `poll_interval_secs`: 轮询间隔（秒）
pub fn spawn_timeout_task(
    review_store: Arc<ReviewStore>,
    poll_interval_secs: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(poll_interval_secs));

        loop {
            interval.tick().await;

            let expired = review_store.process_timeouts().await;
            if !expired.is_empty() {
                info!(
                    "[review] Processed {} timed-out reviews (auto-approved)",
                    expired.len()
                );
            }
        }
    })
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ReviewConfig;

    #[tokio::test]
    async fn test_add_and_get_pending() {
        let config = ReviewConfig::default();
        let store = ReviewStore::new(config);

        let intent = cyber_jianghu_protocol::Intent::new(Uuid::new_v4(), 1, "idle", None);
        let persona = PersonaSummary {
            name: "测试".to_string(),
            gender: "男".to_string(),
            age: 28,
            personality: vec!["沉稳".into()],
            values: vec!["江湖道义".into()],
        };

        let intent_id = store
            .add_pending(intent, Uuid::new_v4(), persona, "测试上下文".to_string())
            .await;

        let pending = store.get_pending().await;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].intent_id, intent_id);
    }

    #[tokio::test]
    async fn test_submit_review() {
        let config = ReviewConfig::default();
        let store = ReviewStore::new(config);

        let intent = cyber_jianghu_protocol::Intent::new(Uuid::new_v4(), 1, "idle", None);
        let persona = PersonaSummary {
            name: "测试".to_string(),
            gender: "男".to_string(),
            age: 28,
            personality: vec![],
            values: vec![],
        };

        let intent_id = store
            .add_pending(intent, Uuid::new_v4(), persona, "".to_string())
            .await;

        let submission = ReviewSubmission {
            result: ReviewDecision::Approved,
            reason: "测试通过".to_string(),
            narrative: Some("测试叙事".to_string()),
        };

        let result = store.submit_review(intent_id, submission).await.unwrap();
        assert_eq!(result.status, ReviewStatus::Approved);

        // 再次提交应该失败
        let submission2 = ReviewSubmission {
            result: ReviewDecision::Rejected,
            reason: "测试拒绝".to_string(),
            narrative: None,
        };
        assert!(matches!(
            store.submit_review(intent_id, submission2).await,
            Err(ReviewError::AlreadyReviewed { .. })
        ));
    }
}
