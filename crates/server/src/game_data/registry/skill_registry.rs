// ============================================================================
// LLM 行为指令 (SKILL.md) 注册表
// ============================================================================
//
// 管理所有 SKILL.md 文件的索引。SKILL.md 是 LLM 行为指引文档，
// 非 RPG 数值技能。详见 tick/processor/skill_mutator.rs。
// ============================================================================

use crate::game_data::registry_or_error;
use crate::game_data::types::skills::SkillDefinition;

/// 技能定义（含 ID）
#[derive(Debug, Clone)]
pub struct SkillWithId {
    /// 技能 ID（如 martial/sword-basic）
    pub skill_id: String,
    /// 技能定义
    pub definition: SkillDefinition,
}

/// 技能注册表
///
/// 提供对技能定义的安全访问
pub struct SkillRegistry;

impl SkillRegistry {
    /// 获取技能定义
    pub fn get(skill_id: &str) -> Option<SkillDefinition> {
        let registry = registry_or_error().ok()?;
        registry.get().skills.get(skill_id).cloned()
    }

    /// 获取所有技能 ID 列表
    pub fn all_ids() -> Vec<String> {
        match registry_or_error() {
            Ok(registry) => registry.get().skills.keys().cloned().collect(),
            Err(_) => vec![],
        }
    }

    /// 获取所有技能定义
    pub fn all() -> Vec<SkillDefinition> {
        match registry_or_error() {
            Ok(registry) => registry.get().skills.values().cloned().collect(),
            Err(_) => vec![],
        }
    }

    /// 获取所有技能定义（含 ID）
    pub fn all_with_id() -> Vec<SkillWithId> {
        match registry_or_error() {
            Ok(registry) => registry
                .get()
                .skills
                .iter()
                .map(|(skill_id, def)| SkillWithId {
                    skill_id: skill_id.clone(),
                    definition: def.clone(),
                })
                .collect(),
            Err(_) => vec![],
        }
    }
}
