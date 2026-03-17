// ============================================================================
// 认知模块
// ============================================================================
//
// 包含叙事化状态描述、结构化输出 Schema 等子模块
//

pub mod narrative;
pub mod output_schema;

// 重新导出核心类型
pub use narrative::{NarrativeEngine, PerceptionNarrative};
pub use output_schema::{
    decision_schema, motivation_schema, perception_schema, planning_schema,
    JsonSchema, SchemaGenerator, SchemaProperty, SchemaPropertyType, SchemaValidator,
    SchemaValidationError,
};
