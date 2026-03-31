//! 审查系统类型定义
//!
//! 用于 Observer Agent 审查 Player Agent 意图的通信协议。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::Intent;

// ============================================================================
// 审查状态
// ============================================================================

/// 审查状态
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ReviewStatus {
    /// 等待审查
    #[default]
    Pending,
    /// 已批准
    Approved,
    /// 已拒绝
    Rejected,
    /// 超时自动通过
    TimeoutApproved,
}

// ============================================================================
// 审查请求/响应
// ============================================================================

/// 人设摘要（用于 API 传输）
///
/// 包含审查决策所需的最小人设信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaSummary {
    /// Agent 名称
    pub name: String,
    /// 性别
    pub gender: String,
    /// 年龄
    pub age: u8,
    /// 性格特点
    pub personality: Vec<String>,
    /// 三观倾向
    pub values: Vec<String>,
}

/// 待审查意图（API 响应用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingReview {
    /// 意图 ID
    pub intent_id: Uuid,
    /// 玩家 Agent ID
    pub agent_id: Uuid,
    /// 意图内容
    pub intent: Intent,
    /// 人设摘要（用于审查决策）
    pub persona_summary: PersonaSummary,
    /// 世界上下文摘要
    pub world_context: String,
    /// 认知链 JSON（用于 ReflectorSoul 本地质量检查）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cognitive_chain: Option<String>,
    /// 创建时间
    pub created_at: DateTime<Utc>,
    /// 审查截止时间
    pub deadline: DateTime<Utc>,
}

/// 审查提交请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewSubmission {
    /// 审查结果
    pub result: ReviewDecision,
    /// 原因说明
    pub reason: String,
    /// 叙事描述（如果批准）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narrative: Option<String>,
}

/// 审查决定
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    /// 批准
    Approved,
    /// 拒绝
    Rejected,
}

/// 审查结果（存储用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewResult {
    /// 意图 ID
    pub intent_id: Uuid,
    /// 最终状态
    pub status: ReviewStatus,
    /// 审查决定（如果有）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<ReviewDecision>,
    /// 原因说明
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// 叙事描述
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narrative: Option<String>,
    /// 审查时间
    pub reviewed_at: DateTime<Utc>,
}

// ============================================================================
// API 响应类型
// ============================================================================

/// 审查列表响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingReviewListResponse {
    /// 待审查列表
    pub pending: Vec<PendingReview>,
    /// 是否有更多
    pub has_more: bool,
}

/// 审查提交响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewSubmissionResponse {
    /// 是否成功
    pub success: bool,
    /// 意图 ID
    pub intent_id: Uuid,
}

/// 审查状态响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewStatusResponse {
    /// 意图 ID
    pub intent_id: Uuid,
    /// 当前状态
    pub status: ReviewStatus,
    /// 审查结果（如果已完成）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ReviewResult>,
}

/// 审查错误响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewErrorResponse {
    /// 错误代码
    pub error: String,
    /// 错误消息
    pub message: String,
}

// ============================================================================
// 错误类型
// ============================================================================

/// 审查错误
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReviewError {
    /// 意图未找到
    IntentNotFound { intent_id: Uuid },
    /// 已审查
    AlreadyReviewed { intent_id: Uuid },
    /// 未授权
    Unauthorized,
    /// 审查已禁用
    ReviewDisabled,
}

impl ReviewError {
    /// 转换为错误响应
    pub fn to_response(&self) -> ReviewErrorResponse {
        match self {
            Self::IntentNotFound { intent_id } => ReviewErrorResponse {
                error: "intent_not_found".to_string(),
                message: format!("Intent {} not found in pending reviews", intent_id),
            },
            Self::AlreadyReviewed { intent_id } => ReviewErrorResponse {
                error: "already_reviewed".to_string(),
                message: format!("Intent {} has already been reviewed", intent_id),
            },
            Self::Unauthorized => ReviewErrorResponse {
                error: "unauthorized".to_string(),
                message: "Invalid or missing authorization token".to_string(),
            },
            Self::ReviewDisabled => ReviewErrorResponse {
                error: "review_disabled".to_string(),
                message: "Review functionality is disabled for this agent".to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_review_status_serde() {
        let status = ReviewStatus::Pending;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"pending\"");

        let parsed: ReviewStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ReviewStatus::Pending);
    }

    #[test]
    fn test_review_decision_serde() {
        let decision = ReviewDecision::Approved;
        let json = serde_json::to_string(&decision).unwrap();
        assert_eq!(json, "\"approved\"");

        let decision = ReviewDecision::Rejected;
        let json = serde_json::to_string(&decision).unwrap();
        assert_eq!(json, "\"rejected\"");
    }

    #[test]
    fn test_persona_summary_serde() {
        let summary = PersonaSummary {
            name: "李四".to_string(),
            gender: "男".to_string(),
            age: 28,
            personality: vec!["沉稳".into(), "重情义".into()],
            values: vec!["江湖道义为先".into()],
        };

        let json = serde_json::to_string(&summary).unwrap();
        let parsed: PersonaSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "李四");
        assert_eq!(parsed.age, 28);
    }

    #[test]
    fn test_review_submission_serde() {
        let submission = ReviewSubmission {
            result: ReviewDecision::Approved,
            reason: "行为符合武侠世界观".to_string(),
            narrative: Some("李四决定出手相助".to_string()),
        };

        let json = serde_json::to_string(&submission).unwrap();
        let parsed: ReviewSubmission = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.result, ReviewDecision::Approved);
        assert!(parsed.narrative.is_some());
    }

    #[test]
    fn test_review_error_response() {
        let error = ReviewError::IntentNotFound {
            intent_id: Uuid::nil(),
        };
        let response = error.to_response();
        assert_eq!(response.error, "intent_not_found");
    }
}
