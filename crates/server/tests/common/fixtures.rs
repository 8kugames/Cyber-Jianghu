//! 测试辅助函数和 fixtures

use cyber_jianghu_protocol::{ActionType, LocationNode};
use uuid::Uuid;
use cyber_jianghu_server::models::{AgentState, Intent, WorldState};

/// 创建测试 Agent
pub fn make_test_agent(agent_id: Uuid, location: &str) -> AgentState {
    AgentState {
        agent_id,
        node_id: location.to_string(),
        is_alive: true,
        inventory_cleared_this_tick: false,
        ..Default::default()
    }
}

/// 创建测试意图
pub fn make_test_intent(agent_id: Uuid, tick_id: i64, action: ActionType) -> Intent {
    Intent::new(agent_id, tick_id, action, None)
}
