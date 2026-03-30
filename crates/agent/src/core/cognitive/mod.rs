// ============================================================================
// 多阶段认知引擎模块
// ============================================================================
//
// 本模块已拆分为多个子模块：
// - stages: 认知阶段定义和响应类型
// - chain: 认知链管理
// - pipeline: 认知流程编排
// - engine: 多阶段认知引擎核心
// ============================================================================

pub mod chain;
pub mod engine;
pub mod narrative;
pub mod output_schema;
pub mod pipeline;
pub mod stages;

// 导出公共接口
pub use stages::{
    CognitiveStage, DecisionResponse, MotivationResponse, PerceptionMotivationPlanningResponse,
    PerceptionResponse, PlanningResponse, StageOutput,
};

pub use chain::CognitiveChain;
pub use engine::{CognitiveEngineConfig, MultiStageCognitiveEngine};
pub use pipeline::{CognitivePipeline, StageProcessor, StageProcessorExt};

// Re-export narrative types for convenience
pub use narrative::{NarrativeConfig, NarrativeEngine, PerceptionNarrative};

// Re-export output_schema types
pub use output_schema::{
    JsonSchema, SchemaGenerator, SchemaProperty, SchemaPropertyType, SchemaValidator,
    SchemaValidationError,
};
