// ============================================================================
// 意图验证器模块
// ============================================================================

pub mod cognitive_validator;
pub mod engine;
mod prompt;
pub mod rule_engine;
mod types;

pub use cognitive_validator::{
    CognitiveEngineWithRetry, CognitiveValidationResult, CognitiveValidator,
};
pub use rule_engine::{
    Rule, RuleCondition, RuleEngine, RuleEngineConfig, RuleType, RuleValidationContext,
    RuleValidationResult,
};
// 为了兼容旧代码，使用类型别名
pub use rule_engine::RuleEngine as RuleEngineValidator;

pub use types::{
    LlmValidationResponse, PersonaInfo, RejectionType, ValidationRequest, ValidationResult,
};

pub use engine::{IntentValidator, Validator};
pub use prompt::{ObserverPrompt, sanitize_for_prompt};
