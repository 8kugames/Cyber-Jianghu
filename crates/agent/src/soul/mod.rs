// ============================================================================
// 三魂系统
// ============================================================================
//
// ActorSoul（人魂/行动之魂）：认知引擎，生成叙事意图（信息隔离，不直接访问 WorldState）
// IntentTranslator（天魂）：将叙事意图翻译为服务端格式化 Intent
// ReflectorSoul（地魂）：Agent 与世界 server 感知和交互的唯一出入口
//   入向：WorldState → NarrativeGenerator → 感知叙事 → 人魂
//   出向：三层审查 Intent（action_type → 规则 → 人设/世界观）→ 提交 server
// ============================================================================

pub mod actor;
pub mod reflector;
pub mod translator;
