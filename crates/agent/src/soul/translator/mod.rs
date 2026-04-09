// ============================================================================
// 天魂 (IntentTranslator) — 意图翻译器
// ============================================================================
//
// 将 ActorSoul（人魂）的自然语言意图翻译为服务端能接受的格式化 Intent。
//
// 三魂架构:
//   人魂 (ActorSoul)     → 叙事意图（"吃馒头充饥"）
//   天魂 (IntentTranslator) → 格式化翻译（action_type + action_data with IDs）
//   地魂 (ReflectorSoul)  → 规则/人设审查
//
// 天魂专注于数据映射，不参与推理。接收叙事 + WorldState，
// 输出 action_type、item_id、target_location 等精确 ID。
// ============================================================================

mod engine;

pub use engine::{IntentTranslator, TranslationResponse};
