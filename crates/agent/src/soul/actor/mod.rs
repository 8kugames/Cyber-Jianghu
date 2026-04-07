// ============================================================================
// ActorSoul（行动之魂/本我）
// ============================================================================
//
// 5 阶段认知引擎（2 次合并 LLM 调用）+ 叙事化，负责生成 Intent
// ============================================================================

pub mod chain;
pub mod engine;
pub mod narrative;
pub mod stages;
pub mod tools;

pub use chain::CognitiveChain;
pub use engine::{CognitiveEngine, CognitiveEngineConfig};
pub use narrative::{NarrativeEngine, PerceptionNarrative};
pub use stages::{CognitiveStage, PerceptionMotivationResponse, PlanDecisionResponse, StageOutput};
pub use tools::{ActorToolExecutor, create_actor_tools};
