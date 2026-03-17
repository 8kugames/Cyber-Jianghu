// ============================================================================
// OpenClaw Cyber-Jianghu MVP 数据模型
// ============================================================================
//
// 本模块定义了MVP阶段所有核心数据结构，包括：
// - Agent基本信息和状态
// - 物品和背包
// - 意图和动作
// - Tick日志
//
// 设计原则：
// 1. 使用清晰的命名，自解释
// 2. 添加详细的文档注释
// 3. 使用Serde进行序列化/反序列化
// 4. 使用合适的类型（UUID、DateTime等）
// 5. 添加必要的验证（如HP范围0-100）
// 6. 保持简洁，不要过度设计
// ============================================================================

// ============================================================================
// 子模块
// ============================================================================

// Agent 相关
pub mod agent;
pub mod state_creation;
pub mod state_impl;
pub mod state_mutation;

// 物品相关
pub mod items;

// 动作相关
pub mod actions;

// Tick 日志相关
pub mod tick;

// API 响应相关
pub mod responses;

// 验证模块
pub mod validation;

// ============================================================================
// Protocol imports
// ============================================================================

use cyber_jianghu_protocol as protocol;

// ============================================================================
// Protocol types (re-export from cyber_jianghu_protocol)
// ============================================================================

pub use protocol::AgentSelfState;
pub use protocol::AvailableAction;
pub use protocol::Entity;
pub use protocol::InitialItem;
pub use protocol::Intent;
pub use protocol::InventoryItem;
pub use protocol::Location;
pub use protocol::WorldEvent;
pub use protocol::WorldState;
pub use protocol::WorldTime;

// ============================================================================
// Re-exports from submodules
// ============================================================================

// Agent 相关
pub use agent::{Agent, AgentState};

// 物品相关
pub use items::ItemType;

// 动作相关
pub use actions::{ActionResult, ActionType, AgentAction};

// Tick 相关
pub use tick::TickLog;

// API 响应相关
pub use responses::{AgentRegisterRequest, AgentRegisterResponse, GameRules, HealthResponse};

// ============================================================================
// 验证模块重导出
// ============================================================================

pub use validation::{
    get_max_agent_name_length, get_max_speak_content_length, get_max_system_prompt_length,
};

// ============================================================================
// 测试和示例
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use super::tick::TickStatus;
    use uuid::Uuid;

    #[test]
    fn test_agent_state_decay() {
        crate::game_data::init_test_registry();

        let mut state = AgentState::new(Uuid::new_v4(), 1);

        // 根据 PRD，白板重生初始值：HP=100, 体力=100, 饥饿=50, 口渴=50
        assert_eq!(state.status.get("hunger").unwrap_or(0), 50);
        assert_eq!(state.status.get("thirst").unwrap_or(0), 50);
        assert_eq!(state.status.get("hp").unwrap_or(0), 100);
        assert!(state.is_alive);

        // 应用衰减（饥饿 -5, 口渴 -5, 体力 +5）
        state.apply_decay(1);
        assert_eq!(state.status.get("hunger").unwrap_or(0), 45);
        assert_eq!(state.status.get("thirst").unwrap_or(0), 45);
        assert_eq!(state.status.get("stamina").unwrap_or(0), 100); // 已经是最大值，不再增加

        // 测试多次衰减导致死亡
        // 测试中的 mock 注册表没有季节系统，所以 modifier 为 1。
        // 从 45 开始，每次 -5。
        let mut loop_count = 0;
        for _ in 0..20 {
            if state.is_alive {
                state.apply_decay(1);
                loop_count += 1;
            }
        }
        println!("Loop count to death: {}", loop_count);
        // We know it starts at 50, and 10 decays of 5 makes it 0.
        // And we trigger death at 0, so is_alive = false.
        assert_eq!(state.status.get("hunger").unwrap_or(0), 0);
        // Note: Because it breaks when hunger reaches 0, thirst might not have been decremented the 10th time!
        assert!(state.status.get("thirst").unwrap_or(0) <= 5);
        assert!(!state.is_alive);
        assert_eq!(state.status.get("hp").unwrap_or(0), 0);
    }

    #[test]
    fn test_agent_state_restore() {
        crate::game_data::init_test_registry();

        let mut state = AgentState::new(Uuid::new_v4(), 1);

        // 应用衰减
        state.apply_decay(1);
        assert_eq!(state.status.get("hunger").unwrap_or(0), 45);
        assert_eq!(state.status.get("thirst").unwrap_or(0), 45);

        // 恢复饥饿值
        state.restore_attribute("hunger", 30);
        assert_eq!(state.status.get("hunger").unwrap_or(0), 75); // 45 + 30 = 75

        // 恢复口渴值
        state.restore_attribute("thirst", 20);
        assert_eq!(state.status.get("thirst").unwrap_or(0), 65); // 45 + 20 = 65

        // 恢复到最大值
        state.restore_attribute("hunger", 50);
        assert_eq!(state.status.get("hunger").unwrap_or(0), 100); // 最大100
    }

    #[test]
    fn test_agent_state_damage() {
        crate::game_data::init_test_registry();

        let mut state = AgentState::new(Uuid::new_v4(), 1);

        // 受到伤害
        state.take_damage(30);
        assert_eq!(state.status.get("hp").unwrap_or(0), 70);
        assert!(state.is_alive);

        // 受到致命伤害
        state.take_damage(100);
        assert_eq!(state.status.get("hp").unwrap_or(0), 0);
        assert!(!state.is_alive);

        // 死亡后无法恢复
        state.restore_attribute("hunger", 50);
        assert_eq!(state.status.get("hunger").unwrap_or(0), 50); // 死亡状态下恢复无效
    }

    #[test]
    fn test_action_type_conversion() {
        // 数据驱动：ActionType 是字符串包装，所有字符串都有效
        let idle = ActionType::new("idle");
        assert_eq!(idle.as_str(), "idle");

        let speak = ActionType::new("speak");
        assert_eq!(speak.as_str(), "speak");

        let custom = ActionType::new("custom_action");
        assert_eq!(custom.as_str(), "custom_action");

        assert_eq!(idle.to_string(), "idle");
        assert_eq!(speak.to_string(), "speak");
    }

    #[test]
    fn test_tick_log() {
        let mut log = TickLog::new(1);
        assert_eq!(log.tick_id, 1);
        assert_eq!(log.status, TickStatus::Running);
        assert!(log.completed_at.is_none());

        log.complete(5, 10);
        assert_eq!(log.status, TickStatus::Completed);
        assert_eq!(log.agents_processed, 5);
        assert_eq!(log.actions_executed, 10);
        assert!(log.completed_at.is_some());
        assert!(log.duration_ms.is_some());
    }
}

// ============================================================================
// Protocol 类型转换（From 实现）
// ============================================================================

// NOTE: From implementations for protocol types are no longer needed
// since these types are now re-exported directly from cyber_jianghu_protocol

#[cfg(test)]
mod jsonb_test;
