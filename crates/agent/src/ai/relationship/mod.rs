// ============================================================================
// 关系记忆系统模块
// ============================================================================
//
// 实现对其他 Agent 的关系记忆：
// - 类型定义（types）：KeyEvent、RelationshipMemory
// - 存储层（store）：RelationshipStore（SQLite 持久化）
//
// 设计原则：
// 1. 关系完全本地化，服务端无法访问
// 2. 支持好感度追踪和关键事件记录
// 3. 为 LLM 提供结构化的关系上下文
// 4. 符合"天道无为"原则，客户端自主管理
// ============================================================================

mod migration;
mod narrative;
mod store;
mod types;

// 重导出常用类型
pub use migration::{MigrationReport, migrate_relationship_descriptions};
pub use narrative::NarrativeGenerator;
pub use store::RelationshipStore;
pub use types::{KeyEvent, RelationshipMemory};
