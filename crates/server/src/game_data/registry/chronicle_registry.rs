use crate::game_data::registry_or_error;
use crate::game_data::types::unified_config::ChronicleRulesData;

/// 群像传记注册表
///
/// 提供对群像传记配置的安全访问
pub struct ChronicleRegistry;

impl ChronicleRegistry {
    /// 获取完整群像传记配置
    pub fn get_config() -> Option<ChronicleRulesData> {
        let registry = registry_or_error().ok()?;
        registry
            .get()
            .game_rules
            .data
            .chronicle
            .clone()
            .or_else(|| Some(ChronicleRulesData::default()))
    }
}
