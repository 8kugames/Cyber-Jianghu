// ============================================================================
// LLM 边界翻译层（已禁用）
// ============================================================================
//
// 设计决策：要求 LLM 精准表述，不按约定的 LLM 自行承担后果。
// Prompt 已明确要求从世界状态数据中直接复制 ID，不做别名翻译。
// ============================================================================

// 本模块已清空。所有别名翻译逻辑已移除：
// - ActionAliasMap: 动作别名 → canonical
// - FieldAliasMap: 字段别名 → canonical
// - EntityAliasMap: 实体别名 → canonical ID
// - EntityTranslationRegistry: 数据驱动实体翻译
//
// LLM 必须精确输出：
// - action_type: 可用动作列表中的 canonical 名称
// - action_data 字段名: 英文原名 (content, item_id, target_location 等)
// - action_data 字段值: 从 WorldState 直接复制的精确 ID
