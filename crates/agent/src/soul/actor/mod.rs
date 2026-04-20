// ============================================================================
// ActorSoul（人魂/行动之魂）
// ============================================================================
//
// 5 阶段认知引擎（2 次合并 LLM 调用）+ 叙事化，生成叙事意图。
// 结构化翻译已消除（人魂直连 WorldState），审核由天魂（ReflectorSoul）负责。
// ============================================================================

pub mod chain;
pub mod chaos;
pub mod engine;
mod engine_prompts; // Prompt 构建方法拆分
pub mod prompt_cache; // Prompt 缓存模块 - 叙事冗余优化
pub mod prompt_template; // Prompt 模板配置加载器
pub mod stages;
pub mod summary_window; // 滑动上下文窗口 - 叙事冗余优化
pub mod translation; // 中文 LLM 边界翻译层

pub use chain::CognitiveChain;
pub use chaos::{ChaosConfig, ChaosGenerator};
pub use engine::{CognitiveEngine, CognitiveEngineConfig};
pub use prompt_cache::PromptCache;
pub use stages::{CognitiveStage, PerceptionMotivationResponse, StageOutput};
pub use summary_window::{NarrativeSummary, NarrativeSummaryWindow};
