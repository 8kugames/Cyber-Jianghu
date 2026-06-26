// ============================================================================
// Reward 配置注册表（配置安全访问层）
// ============================================================================
//
// 仿 TimeRegistry::get_config() 模式。
// reward.yaml 为强制配置（loader 缺失即 Err），故正常初始化后必然存在；
// 返回 Option 仅因 registry_or_error() 本身可能未初始化（启动早期）。
// ============================================================================

use crate::game_data::registry::global::registry;
use crate::game_data::types::reward::RewardConfig;

/// Reward 配置访问器
pub struct RewardRegistry;

impl RewardRegistry {
    /// 获取 reward 配置
    pub fn get_config() -> Option<RewardConfig> {
        registry().map(|r| r.get().reward.clone())
    }
}
