// ============================================================================
// 对话模块
// ============================================================================
//
// 提供 Agent 之间的对话功能
//
// ============================================================================

mod dialogue_handler;
mod session;
mod session_manager;
mod types;

pub use session_manager::DialogueManager;
pub use types::DialogueResponse;
