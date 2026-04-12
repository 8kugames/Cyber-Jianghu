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
pub mod prompt_cache; // Prompt 缓存模块 - 叙事冗余优化
pub mod stages;
pub mod summary_window; // 滑动上下文窗口 - 叙事冗余优化
pub mod tools;

pub use chain::CognitiveChain;
pub use engine::{CognitiveEngine, CognitiveEngineConfig};
pub use narrative::{NarrativeEngine, PerceptionNarrative};
pub use prompt_cache::{ChangeMarkers, PromptCache};
pub use stages::{CognitiveStage, PerceptionMotivationResponse, PlanDecisionResponse, StageOutput};
pub use summary_window::{NarrativeSummary, NarrativeSummaryWindow};
// tools: ActorToolExecutor/create_actor_tools 保留供测试和未来扩展，不公开导出
