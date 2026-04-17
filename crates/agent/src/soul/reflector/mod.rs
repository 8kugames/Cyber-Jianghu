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

pub mod cognitive_validator;
pub mod leak_detector;
pub mod narrative_generator;
pub mod prompt;
pub mod rule_engine;
pub mod store;
pub mod types;
pub mod validator;

pub use leak_detector::LeakDetector;
pub use narrative_generator::NarrativeGenerator;
pub use prompt::{ObserverPrompt, sanitize_for_prompt};
pub use rule_engine::RuleEngine as RuleEngineValidator;
pub use rule_engine::{
    Rule, RuleCondition, RuleEngine, RuleEngineConfig, RuleType, RuleValidationContext,
    RuleValidationResult,
};
pub use store::{PendingReview, PendingReviewEntry, ReviewDecision, ReviewStatus, ReviewStore};
pub use types::{
    LlmValidationResponse, PersonaInfo, RejectionReason, RejectionType, ValidationRequest,
    ValidationResult,
};
pub use validator::{ReflectorSoul, Validator};
