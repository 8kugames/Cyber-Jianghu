// ============================================================================
// AgentState 构造函数和基本方法
// ============================================================================

use chrono::Utc;
use uuid::Uuid;

use super::AgentState;

impl AgentState {
    /// 创建新的Agent状态（白板重生状态）
    ///
    /// 从统一配置读取所有属性的初始值，使用组件化架构
    pub fn new(agent_id: Uuid, tick_id: i64) -> Self {
        // 获取全局配置注册表
        let registry = crate::game_data::registry_or_error().unwrap_or_else(|e| {
            panic!("AgentState::new() 需要初始化注册表: {}", e);
        });
        let data = registry.get();

        // 从统一配置创建 StatusComponent（状态值）
        let status =
            crate::game_data::types::StatusComponent::from_unified_config(&data.attributes);

        // 从统一配置创建 AttributeComponent（先天属性）
        let primary_attributes =
            crate::game_data::types::AttributeComponent::from_unified_config(&data.attributes);

        // 从配置获取出生点
        let node_id = data
            .game_rules
            .data
            .agent_state
            .location
            .spawn_location
            .clone();

        Self {
            id: 0, // 数据库自动生成
            agent_id,
            tick_id,
            primary_attributes,
            status,
            node_id,
            is_alive: true,
            inventory_cleared_this_tick: false,
            created_at: Utc::now(),
        }
    }

    /// 获取整数属性值（优先状态值，其次先天属性）
    pub fn get_i32(&self, name: &str) -> Option<i32> {
        // 先从状态值查找
        if let Some(val) = self.status.get(name) {
            return Some(val);
        }
        // 再从先天属性查找
        self.primary_attributes.get_value(name)
    }
}
