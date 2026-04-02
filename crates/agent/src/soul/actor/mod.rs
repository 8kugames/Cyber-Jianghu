// ============================================================================
// ActorSoul（行动之魂/本我）
// ============================================================================
//
// 多阶段认知引擎 + 叙事化，负责生成 Intent
// ============================================================================

pub mod chain;
pub mod engine;
pub mod narrative;
pub mod stages;

pub use chain::CognitiveChain;
pub use engine::{CognitiveEngine, CognitiveEngineConfig};
pub use narrative::{NarrativeEngine, PerceptionNarrative};
pub use stages::{
    CognitiveStage, PerceptionMotivationResponse, PlanDecisionResponse, StageOutput,
};
