// ============================================================================
// ReflectorSoul（地魂/反思之魂）
// ============================================================================
//
// 意图审查引擎，三魂架构中的第三层。
// 天魂（IntentTranslator）翻译的格式化 Intent 必须经过地魂审查才能提交到服务端。
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
