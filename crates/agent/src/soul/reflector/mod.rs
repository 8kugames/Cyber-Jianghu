// ============================================================================
// ReflectorSoul（天魂/守护之魂）
// ============================================================================
//
// 天魂负责审核 Intent，确保合法后才提交 server。
//
// 出向（审核）：人魂 Intent → 天魂四层审查 → 提交 server
//
// 四层审查：Layer 0 目标硬性校验 → Layer 1 action_type 合法性
//          → Layer 2 RuleEngine 规则校验 → Layer 3 LLM 人设/世界观审查
// ============================================================================

pub mod cognitive_validator;
pub(crate) mod hard_logic;
pub mod prompt;
pub mod rule_engine;
pub mod types;
pub mod validator;

pub use prompt::{ReflectorPrompt, sanitize_for_prompt};
pub use rule_engine::{
    Rule, RuleCondition, RuleEngine, RuleEngineConfig, RuleType, RuleValidationContext,
    RuleValidationResult,
};
pub use types::{
    LayerResult, LlmValidationResponse, PersonaInfo, PipelineValidationResult, RejectionReason,
    RejectionType, ValidationRequest, ValidationResult, ValidationRuntimeConfig,
};
pub use validator::{ReflectorSoul, Validator};
