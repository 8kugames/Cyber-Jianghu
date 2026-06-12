use thiserror::Error;
use uuid::Uuid;

/// 统一的游戏错误类型
///
/// 使用 thiserror 宏自动生成 Display 和 Error 实现
/// 替代原先分散在各个模块中的 ActionError, DialogueError, SkillError, ItemError, ClientError
#[derive(Debug, Error)]
pub enum GameError {
    // ========== 动作相关错误 (原 ActionError) ==========
    /// Agent 已死亡
    #[error("Agent 已死亡，无法执行此动作。请重新转生入世。")]
    AgentDead { agent_id: Uuid },

    /// 目标 Agent 未找到
    #[error("Target agent {target_id} not found")]
    TargetNotFound { target_id: Uuid },

    /// 目标 Agent 已死亡
    #[error("Target agent {target_id} is dead")]
    TargetDead { target_id: Uuid },

    /// 无效的动作数据
    #[error("Invalid action data: {reason}")]
    InvalidActionData { reason: String },

    /// 动作执行失败
    #[error("Action failed: {0}")]
    Action(String),

    // ========== 对话系统错误 (原 DialogueError) ==========
    /// Agent 已经在对话中
    #[error("Agent {agent_id} is already in dialogue")]
    AlreadyInDialogue { agent_id: Uuid },

    /// 目标 Agent 忙碌（正在对话中）
    #[error("Target agent {target_agent_id} is busy in dialogue")]
    TargetBusy { target_agent_id: Uuid },

    /// 会话不存在
    #[error("Session {session_id} not found")]
    SessionNotFound { session_id: String },

    /// Agent 不是会话参与者
    #[error("Agent {agent_id} is not a participant in session {session_id}")]
    NotParticipant { agent_id: Uuid, session_id: String },

    /// 会话未激活
    #[error("Session {session_id} is not active")]
    SessionNotActive { session_id: String },

    /// 消息数量已达上限
    #[error("Message limit reached for session {session_id}")]
    MessageLimitReached { session_id: String },

    /// 对话系统错误（通用）
    #[error("Dialogue error: {0}")]
    Dialogue(String),

    // ========== 技能系统错误 (原 SkillError) ==========
    /// 技能序列化错误
    #[error("Skill serialization error: {0}")]
    SkillSerializationError(String),

    /// 技能解析错误
    #[error("Skill parse error: {0}")]
    SkillParseError(String),

    /// 技能 IO 错误
    #[error("Skill IO error: {0}")]
    SkillIoError(String),

    /// 技能执行错误
    #[error("Skill execution error: {0}")]
    SkillExecutionError(String),

    /// 技能未找到
    #[error("Skill not found: {0}")]
    SkillNotFound(String),

    /// 技能系统错误（通用）
    #[error("Skill error: {0}")]
    Skill(String),

    // ========== 物品系统错误 (原 ItemError) ==========
    /// 物品不存在
    #[error("物品不存在: {0}")]
    ItemNotFound(String),

    /// 物品不可使用
    #[error("物品不可使用: {0}")]
    ItemNotUsable(String),

    /// 物品不可装备
    #[error("物品不可装备: {0}")]
    ItemNotEquippable(String),

    /// 效果应用失败
    #[error("效果应用失败: {0}")]
    EffectApplyFailed(String),

    // ========== 客户端错误 (原 ClientError) ==========
    /// WebSocket 连接失败
    #[error("WebSocket connection failed: {0}")]
    ClientConnectionFailed(String),

    /// WebSocket 发送失败
    #[error("WebSocket send failed: {0}")]
    ClientSendFailed(String),

    /// WebSocket 接收失败
    #[error("WebSocket receive failed: {0}")]
    ClientReceiveFailed(String),

    /// 消息解析失败
    #[error("Failed to parse message: {0}")]
    ClientParseFailed(String),

    /// 认证失败
    #[error("Authentication failed: {0}")]
    ClientAuthFailed(String),

    /// 连接已断开
    #[error("Connection closed")]
    ClientConnectionClosed,

    // ========== 通用错误 ==========
    /// 服务器尚未开始接受意图
    #[error("服务器尚未开始接受意图")]
    NotAccepting,

    /// Tick ID 不匹配
    #[error(
        "Intent tick_id {intent_tick_id} 不匹配当前 tick {current_tick_id}。请提交当前 tick 的意图。"
    )]
    TickMismatch {
        intent_tick_id: i64,
        current_tick_id: i64,
    },

    /// 验证失败
    #[error("Validation failed: {0}")]
    Validation(String),

    /// 资源未找到
    #[error("Not found: {0}")]
    NotFound(String),

    /// 操作失败
    #[error("Operation failed: {0}")]
    Operation(String),

    /// 未知错误
    #[error("Unknown error: {0}")]
    Unknown(String),
}

// 实现从 String 到 GameError 的转换
impl From<String> for GameError {
    fn from(msg: String) -> Self {
        GameError::Unknown(msg)
    }
}

impl From<&str> for GameError {
    fn from(msg: &str) -> Self {
        GameError::Unknown(msg.to_string())
    }
}

impl GameError {
    /// 提取关联的 tick_id（仅 TickMismatch 有值）
    pub fn current_tick_id(&self) -> Option<i64> {
        match self {
            GameError::TickMismatch {
                current_tick_id, ..
            } => Some(*current_tick_id),
            _ => None,
        }
    }
}

impl GameError {
    /// 返回机器可读的错误码字符串
    pub fn error_code(&self) -> &'static str {
        match self {
            GameError::AgentDead { .. } => crate::ERROR_CODE_AGENT_DEAD,
            GameError::NotAccepting => crate::ERROR_CODE_NOT_ACCEPTING,
            GameError::TickMismatch { .. } => crate::ERROR_CODE_TICK_MISMATCH,
            GameError::AlreadyInDialogue { .. }
            | GameError::TargetBusy { .. }
            | GameError::SessionNotFound { .. }
            | GameError::NotParticipant { .. }
            | GameError::SessionNotActive { .. }
            | GameError::MessageLimitReached { .. }
            | GameError::Dialogue(_) => crate::ERROR_CODE_DIALOGUE_FAILED,
            GameError::InvalidActionData { .. } | GameError::Action(_) => {
                crate::ERROR_CODE_ACTION_FAILED
            }
            _ => "unknown",
        }
    }
}

// 实现从 std::io::Error 到 GameError::SkillIoError 的转换
impl From<std::io::Error> for GameError {
    fn from(err: std::io::Error) -> Self {
        GameError::SkillIoError(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_game_error_agent_dead() {
        let agent_id = Uuid::new_v4();
        let err = GameError::AgentDead { agent_id };
        assert!(err.to_string().contains("已死亡"));
    }

    #[test]
    fn test_game_error_target_not_found() {
        let target_id = Uuid::new_v4();
        let err = GameError::TargetNotFound { target_id };
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn test_game_error_target_dead() {
        let target_id = Uuid::new_v4();
        let err = GameError::TargetDead { target_id };
        assert!(err.to_string().contains("is dead"));
    }

    #[test]
    fn test_game_error_invalid_action_data() {
        let err = GameError::InvalidActionData {
            reason: "missing field".to_string(),
        };
        assert!(err.to_string().contains("missing field"));
    }

    #[test]
    fn test_game_error_action() {
        let err = GameError::Action("test action failed".to_string());
        assert_eq!(err.to_string(), "Action failed: test action failed");
    }

    #[test]
    fn test_game_error_dialogue() {
        let err = GameError::Dialogue("dialogue failed".to_string());
        assert_eq!(err.to_string(), "Dialogue error: dialogue failed");
    }

    #[test]
    fn test_game_error_skill() {
        let err = GameError::Skill("skill error".to_string());
        assert_eq!(err.to_string(), "Skill error: skill error");
    }

    #[test]
    fn test_game_error_skill_serialization() {
        let err = GameError::SkillSerializationError("YAML error".to_string());
        assert_eq!(err.to_string(), "Skill serialization error: YAML error");
    }

    #[test]
    fn test_game_error_skill_parse() {
        let err = GameError::SkillParseError("invalid format".to_string());
        assert_eq!(err.to_string(), "Skill parse error: invalid format");
    }

    #[test]
    fn test_game_error_skill_io() {
        let err = GameError::SkillIoError("file not found".to_string());
        assert_eq!(err.to_string(), "Skill IO error: file not found");
    }

    #[test]
    fn test_game_error_skill_execution() {
        let err = GameError::SkillExecutionError("command failed".to_string());
        assert_eq!(err.to_string(), "Skill execution error: command failed");
    }

    #[test]
    fn test_game_error_skill_not_found() {
        let err = GameError::SkillNotFound("my-skill".to_string());
        assert_eq!(err.to_string(), "Skill not found: my-skill");
    }

    #[test]
    fn test_game_error_item_not_found() {
        let err = GameError::ItemNotFound("sword".to_string());
        assert_eq!(err.to_string(), "物品不存在: sword");
    }

    #[test]
    fn test_game_error_item_not_usable() {
        let err = GameError::ItemNotUsable("银子".to_string());
        assert_eq!(err.to_string(), "物品不可使用: 银子");
    }

    #[test]
    fn test_game_error_item_not_equippable() {
        let err = GameError::ItemNotEquippable("馒头".to_string());
        assert_eq!(err.to_string(), "物品不可装备: 馒头");
    }

    #[test]
    fn test_game_error_effect_apply_failed() {
        let err = GameError::EffectApplyFailed("属性 satiation 不存在".to_string());
        assert_eq!(err.to_string(), "效果应用失败: 属性 satiation 不存在");
    }

    #[test]
    fn test_game_error_client_connection_failed() {
        let err = GameError::ClientConnectionFailed("connection refused".to_string());
        assert_eq!(
            err.to_string(),
            "WebSocket connection failed: connection refused"
        );
    }

    #[test]
    fn test_game_error_client_send_failed() {
        let err = GameError::ClientSendFailed("broken pipe".to_string());
        assert_eq!(err.to_string(), "WebSocket send failed: broken pipe");
    }

    #[test]
    fn test_game_error_client_receive_failed() {
        let err = GameError::ClientReceiveFailed("connection reset".to_string());
        assert_eq!(
            err.to_string(),
            "WebSocket receive failed: connection reset"
        );
    }

    #[test]
    fn test_game_error_client_parse_failed() {
        let err = GameError::ClientParseFailed("invalid JSON".to_string());
        assert_eq!(err.to_string(), "Failed to parse message: invalid JSON");
    }

    #[test]
    fn test_game_error_client_auth_failed() {
        let err = GameError::ClientAuthFailed("invalid token".to_string());
        assert_eq!(err.to_string(), "Authentication failed: invalid token");
    }

    #[test]
    fn test_game_error_client_connection_closed() {
        let err = GameError::ClientConnectionClosed;
        assert_eq!(err.to_string(), "Connection closed");
    }

    #[test]
    fn test_game_error_validation() {
        let err = GameError::Validation("invalid input".to_string());
        assert_eq!(err.to_string(), "Validation failed: invalid input");
    }

    #[test]
    fn test_game_error_not_found() {
        let err = GameError::NotFound("agent".to_string());
        assert_eq!(err.to_string(), "Not found: agent");
    }

    #[test]
    fn test_game_error_operation() {
        let err = GameError::Operation("database error".to_string());
        assert_eq!(err.to_string(), "Operation failed: database error");
    }

    #[test]
    fn test_game_error_unknown() {
        let err = GameError::Unknown("something went wrong".to_string());
        assert_eq!(err.to_string(), "Unknown error: something went wrong");
    }

    #[test]
    fn test_from_string() {
        let err: GameError = "error message".into();
        assert!(matches!(err, GameError::Unknown(_)));
    }
}
