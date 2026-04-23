// ============================================================================
// 社交系统（关系 + 对话）
// ============================================================================

mod dialogue;
mod relationship;
mod relationship_migration;
mod relationship_narrative;
mod relationship_types;

pub use dialogue::{DialogueClient, DialogueEventHandler};
pub use relationship::RelationshipStore;
pub use relationship_migration::{MigrationReport, migrate_relationship_descriptions};
pub use relationship_narrative::NarrativeGenerator;
pub use relationship_types::{get_relationship_level, KeyEvent, RelationshipMemory};
