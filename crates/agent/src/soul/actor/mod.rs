// ============================================================================
// ActorSoul（人魂/行动之魂）
// ============================================================================
//
// 5 阶段认知引擎（2 次合并 LLM 调用）+ 叙事化，生成叙事意图。
// 结构化翻译由天魂（IntentTranslator）负责，审查由地魂（ReflectorSoul）负责。
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
