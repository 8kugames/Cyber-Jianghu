// ============================================================================
// ActorSoul（人魂/行动之魂）
// ============================================================================
//
// 认知引擎直连 WorldState，单次 LLM 调用生成结构化 Intent。
// 地魂 tool-calling 按需加载技能详情（progressive disclosure）。
// 审核由天魂（ReflectorSoul）三层审查负责。
// ============================================================================

pub mod chain;
pub mod chaos;
pub mod engine;
mod engine_prompts; // Prompt 构建方法拆分
pub mod prompt_cache; // Prompt 缓存模块 - 叙事冗余优化
pub mod prompt_template; // Prompt 模板配置加载器
pub mod stages;
pub mod summary_window; // 滑动上下文窗口 - 叙事冗余优化
pub mod translation; // 翻译层已禁用（设计决策：要求 LLM 精准表述）

pub use chain::CognitiveChain;
pub use chaos::{ChaosConfig, ChaosGenerator};
pub use engine::{CognitiveEngine, CognitiveEngineConfig};
pub use prompt_cache::PromptCache;
pub use stages::{CognitiveStage, PerceptionMotivationResponse, StageOutput};
pub use summary_window::{NarrativeSummary, NarrativeSummaryWindow};
