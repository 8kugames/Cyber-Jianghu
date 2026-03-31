// ============================================================================
// Service Layer - 业务逻辑层
// ============================================================================
//
// 提取 handlers.rs 中的业务逻辑，保持 handlers 专注于 HTTP 处理
// 遵循单一职责原则

mod memory;
mod relationship;

pub use memory::{MemoryService, memories_to_json_response, memory_to_json, search_result_to_json};
pub use relationship::RelationshipService;
