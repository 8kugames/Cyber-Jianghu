// ============================================================================
// 地魂 (IntentTranslator) — 能力之魂（Phase 3 激活 tool calling）
// ============================================================================
//
// 旧职责：将人魂叙事意图翻译为格式化 Intent（人魂直连 WorldState 后已消除）。
// 新职责：tool calling 能力池（记忆搜索、关系查询、属性查询、物品查询）。
//
// 三魂架构:
//   人魂 (ActorSoul)       → 直连 WorldState，输出结构化 Intent
//   天魂 (ReflectorSoul)   → 三层审核（action_type → 规则 → 人设）
//   地魂 (IntentTranslator) → tool calling 能力池
//
// Phase 4 将清理翻译相关代码。
// ============================================================================

mod engine;

pub use engine::{IntentTranslator, MultiTranslationResult, TranslationResponse};
