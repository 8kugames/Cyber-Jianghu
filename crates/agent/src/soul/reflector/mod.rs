// ============================================================================
// ReflectorSoul（反思之魂/超我）
// ============================================================================
//
// 意图验证 + 审查存储，负责审查 ActorSoul 生成的 Intent
// ============================================================================

pub mod cognitive_validator;
pub mod prompt;
pub mod rule_engine;
pub mod store;
pub mod types;
pub mod validator;

pub use cognitive_validator::{
    CognitiveEngineWithRetry, CognitiveValidationResult, CognitiveValidator,
};
pub use prompt::{ObserverPrompt, sanitize_for_prompt};
pub use rule_engine::RuleEngine as RuleEngineValidator;
pub use rule_engine::{
    Rule, RuleCondition, RuleEngine, RuleEngineConfig, RuleType, RuleValidationContext,
    RuleValidationResult,
};
pub use store::{PendingReview, PendingReviewEntry, ReviewDecision, ReviewStatus, ReviewStore};
pub use types::{
    LlmValidationResponse, PersonaInfo, RejectionType, ValidationRequest, ValidationResult,
};
pub use validator::{IntentValidator, Validator};
