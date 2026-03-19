// ============================================================================
// 对话类型定义
// ============================================================================
//
// 定义对话系统使用的公共类型
//
// ============================================================================

use uuid::Uuid;

/// 对话响应
///
/// 表示对话处理后的响应类型
#[derive(Debug)]
pub enum DialogueResponse {
    /// 请求已转发给目标 Agent
    RequestForwarded {
        session_id: String,
        target_agent_id: Uuid,
    },

    /// 会话已建立
    SessionStarted {
        session_id: String,
        agent_a: Uuid,
        agent_b: Uuid,
    },

    /// 会话请求被拒绝
    SessionRejected {
        session_id: String,
        rejected_by: Uuid,
        requester: Uuid,
    },

    /// 内容已转发
    ContentForward {
        session_id: String,
        from_agent_id: Uuid,
        to_agent_id: Uuid,
    },

    /// 会话已结束
    SessionEnded {
        session_id: String,
        ended_by: Uuid,
        other_participant: Uuid,
    },
}
