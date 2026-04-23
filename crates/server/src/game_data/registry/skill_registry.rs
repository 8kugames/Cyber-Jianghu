// ============================================================================
// 技能注册表
// ============================================================================

use crate::game_data::registry_or_error;
use crate::game_data::types::skills::SkillDefinition;

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
}
