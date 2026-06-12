// ============================================================================
// ReflectorSoul（天魂/守护之魂）
// ============================================================================
//
// 天魂负责审核 Intent，确保合法后才提交 server。
//
// 出向（审核）：人魂 Intent → 天魂三层审查 → 提交 server
//
// 三层审查：action_type 合法性 → RuleEngine 规则校验 → LLM 人设/世界观审查
// ============================================================================

pub mod atomic_gate;
pub mod cognitive_validator;
pub mod governance;
pub mod prompt;
pub mod rule_engine;
pub mod types;
pub mod validator;

pub use atomic_gate::check_atomicity;
pub use governance::{EvaluatorDecision, SelfEvaluator, SelfEvaluatorOutput};
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
