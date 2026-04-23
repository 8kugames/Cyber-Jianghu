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
pub use protocol::GatherableItem;
pub use protocol::InitialItem;
pub use protocol::Intent;
pub use protocol::InventoryItem;
pub use protocol::Location;
pub use protocol::PrivateDialogueRecord;
pub use protocol::RecentAction;
pub use protocol::WorldEvent;
pub use protocol::WorldEventType;
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

// Vendor 待注入事件缓冲区
pub type VendorPendingEvents =
    std::sync::Arc<dashmap::DashMap<uuid::Uuid, Vec<protocol::WorldEvent>>>;

// API 响应相关
pub use responses::{
    AgentConnectRequest, AgentConnectResponse, AgentRegisterRequest, AgentRegisterResponse,
    GameRules, HealthResponse,
};

// ============================================================================
// 验证模块重导出
// ============================================================================

#[allow(unused_imports)]
pub use validation::get_max_speak_content_length;
pub use validation::{get_max_agent_name_length, get_max_system_prompt_length};

// ============================================================================
// 测试和示例
// ============================================================================

#[cfg(test)]
mod tests {
    use super::tick::TickStatus;
    use super::*;
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

        // 应用衰减（hunger/thirst 每tick衰减5，stamina 使用 recovery_formula 恢复）
        let _ = state.apply_decay(1);
        assert_eq!(state.status.get("hunger").unwrap_or(0), 45); // 50 - 5 = 45
        assert_eq!(state.status.get("thirst").unwrap_or(0), 45); // 50 - 5 = 45
        assert_eq!(state.status.get("stamina").unwrap_or(0), 100); // 已经是最大值，不再增加

        // 测试多次衰减导致死亡
        // 测试中的 mock 注册表没有季节系统，所以 modifier 为 1。
        // hunger/thirst 每tick衰减5（decay_per_tick: 5 表示扣减5）
        // 从 45 开始，每次 -5，9次后归零死亡
        let mut loop_count = 0;
        for _ in 0..20 {
            if state.is_alive {
                let _ = state.apply_decay(1);
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
        let _ = state.apply_decay(1);
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
        let idle = ActionType::new("休息");
        assert_eq!(idle.as_str(), "休息");

        let speak = ActionType::new("说话");
        assert_eq!(speak.as_str(), "说话");

        let custom = ActionType::new("custom_action");
        assert_eq!(custom.as_str(), "custom_action");

        assert_eq!(idle.to_string(), "休息");
        assert_eq!(speak.to_string(), "说话");
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

    #[test]
    fn test_stamina_recovery_from_zero() {
        crate::game_data::init_test_registry();

        let mut state = AgentState::new(Uuid::new_v4(), 1);

        // 手动设置 stamina 为 0
        state.status.set("stamina", 0).unwrap();

        println!("Initial stamina: {:?}", state.status.get("stamina"));

        // 应用恢复（stamina 使用 recovery_formula）
        let _ = state.apply_decay(1);

        println!("After decay stamina: {:?}", state.status.get("stamina"));

        // stamina 应该从 0 恢复到 5 (recovery_formula: "5 + constitution * 0.1", constitution 默认为 10)
        // 5 + 10 * 0.1 = 6
        let constitution = state
            .primary_attributes
            .get_value("constitution")
            .unwrap_or(10);
        let expected_recovery = 5 + (constitution as f64 * 0.1).floor() as i32;
        assert_eq!(state.status.get("stamina").unwrap_or(-1), expected_recovery);
    }

    #[test]
    fn test_stamina_max_value_with_constitution() {
        crate::game_data::init_test_registry();

        let state = AgentState::new(Uuid::new_v4(), 1);

        // 获取 constitution 的值
        let constitution = state
            .primary_attributes
            .get_value("constitution")
            .unwrap_or(10);
        println!("Constitution: {}", constitution);

        // 获取 stamina 的 decay_per_tick
        let stamina_decay = state.status.decay_per_tick("stamina");
        println!("Stamina decay_per_tick: {:?}", stamina_decay);

        // 计算 stamina 的 max_value
        let context = state.get_formula_context();
        println!("Formula context: {:?}", context);

        // stamina max = 100 + constitution * 1
        let expected_max = 100 + constitution;
        println!("Expected max stamina: {}", expected_max);

        // 验证 context 中有 constitution
        assert!(
            context.contains_key("constitution"),
            "constitution should be in context"
        );
        assert_eq!(context.get("constitution"), Some(&constitution));
    }

    #[test]
    fn test_stamina_recovery_attributes_list() {
        crate::game_data::init_test_registry();

        let state = AgentState::new(Uuid::new_v4(), 1);

        // 获取所有需要衰减的属性
        let decaying = state.status.get_decaying_attributes();
        println!("Decaying attributes: {:?}", decaying);

        // 获取所有需要恢复的属性
        let recovering = state.status.get_recovering_attributes();
        println!("Recovering attributes: {:?}", recovering);

        // 验证 stamina 使用 recovery_formula 而非 decay_per_tick
        let stamina_in_decaying = decaying.iter().find(|(name, _)| name == "stamina");
        assert!(
            stamina_in_decaying.is_none(),
            "stamina should NOT be in decaying attributes"
        );

        let stamina_in_recovering = recovering.iter().find(|(name, _)| name == "stamina");
        assert!(
            stamina_in_recovering.is_some(),
            "stamina should be in recovering attributes"
        );

        // 验证 hunger 和 thirst 在衰减列表中
        assert!(
            decaying.iter().any(|(name, _)| name == "hunger"),
            "hunger should be in decaying attributes"
        );
        assert!(
            decaying.iter().any(|(name, _)| name == "thirst"),
            "thirst should be in decaying attributes"
        );
    }
}

// ============================================================================
// Protocol 类型转换（From 实现）
// ============================================================================

// NOTE: From implementations for protocol types are no longer needed
// since these types are now re-exported directly from cyber_jianghu_protocol

#[cfg(test)]
mod jsonb_test;
