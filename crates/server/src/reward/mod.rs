// ============================================================================
// 生存 Reward 模块（天道账本）
// ============================================================================
//
// 哲学锚点：天道无为。reward 纯锚定生存因果（寿数 + 死亡 penalty）。
// - 身家不计入（决策②）
// - 死因 penalty 统一（决策③：死就是死，无高下）
// - 每游戏日结算（决策④，复用 scheduler.rs:896 日边界检测）
// - 配置驱动（reward.yaml，零硬编码）
//
// 数据来源全部是 server 侧物理事实，零新增数据通道。
// ============================================================================

pub mod daily;
pub mod lifetime;
pub mod periodic;
pub mod types;

pub use daily::{compute_daily_reward, settle_daily};
pub use lifetime::settle_lifetime;
pub use periodic::settle_periodic;
pub use types::{DailyReward, LifetimeReward, PeriodReward};

#[cfg(test)]
mod tests {
    use super::*;

    /// P1-2 来源断言式测试：证明 reward 各分量严格来自配置，无硬编码。
    ///
    /// 通过 init_test_registry 初始化真实配置（survival_score=1.0,
    /// satiation_weight=0.25, max_value=100），构造 agent 状态，
    /// 断言生理分量 = satiation/max × weight（配置派生值）。
    #[test]
    fn test_daily_reward_sources_from_config() {
        crate::game_data::init_test_registry();

        // 构造 agent 状态：satiation=80, hydration=60, alive
        // test_utils 的 satiation max_value_formula="100"，故 max=100
        let agent = make_test_agent_state(80, 60, true);

        let reward = compute_daily_reward(&agent, 1, None).expect("reward should compute");

        // P1-4: 生理分量 = 80/100×0.25 + 60/100×0.25 = 0.20 + 0.15 = 0.35
        assert!(
            (reward.physiological - 0.35).abs() < 0.001,
            "physiological 应来自配置派生值，got {}",
            reward.physiological
        );
        // 生存分量 = cfg.daily.survival_score = 1.0
        assert!(
            (reward.survival - 1.0).abs() < 0.001,
            "survival 应等于 cfg.daily.survival_score"
        );
        // total = 1.0 + 0.35 = 1.35
        assert!(
            (reward.total - 1.35).abs() < 0.001,
            "total 应等于各分量之和，got {}",
            reward.total
        );
    }

    /// P1-2 反例：死亡 agent 的 survival 分量应为 0（证明 survival 来自 is_alive，非硬编码 +1）
    #[test]
    fn test_daily_reward_dead_agent_zero_survival() {
        crate::game_data::init_test_registry();
        let agent = make_test_agent_state(80, 60, false);
        let reward = compute_daily_reward(&agent, 1, None).expect("reward should compute");
        assert!(
            (reward.survival - 0.0).abs() < 0.001,
            "死亡 agent survival 应为 0，got {}",
            reward.survival
        );
    }

    /// P1-2 反例：篡改 satiation 值，physiological 应随之改变（证明非硬编码）
    #[test]
    fn test_daily_reward_physiological_tracks_value() {
        crate::game_data::init_test_registry();
        let agent_low = make_test_agent_state(20, 20, true);
        let agent_high = make_test_agent_state(100, 100, true);
        let r_low = compute_daily_reward(&agent_low, 1, None).unwrap();
        let r_high = compute_daily_reward(&agent_high, 1, None).unwrap();
        assert!(
            r_high.physiological > r_low.physiological,
            "高饱食/饱饮应得更高生理分量（证明非硬编码），low={} high={}",
            r_low.physiological,
            r_high.physiological
        );
    }

    /// P1-6: 死因不参与 penalty——reward 模块不按死因差异化（由 lifetime 统一 -50 保证）
    /// 此处验证 compute_daily_reward 无死因逻辑：天魂 None 时不影响分量
    #[test]
    fn test_daily_reward_tianhun_none_zero() {
        crate::game_data::init_test_registry();
        let agent = make_test_agent_state(80, 60, true);
        let reward = compute_daily_reward(&agent, 1, None).unwrap();
        assert!(
            reward.tianhun_judgment.is_none(),
            "无天魂数据时 judgment 应为 None"
        );
    }

    /// 构造测试用 AgentState（satiation/hydration/alive 可控）
    ///
    /// 复用 AgentState::new 工厂方法（内部从 registry 读配置），
    /// 再 set 覆盖属性值，避免手搓复杂结构体。
    fn make_test_agent_state(
        satiation: i32,
        hydration: i32,
        alive: bool,
    ) -> crate::models::AgentState {
        let mut state = crate::models::AgentState::new(uuid::Uuid::new_v4(), 720);
        let _ = state.status.set("satiation", satiation);
        let _ = state.status.set("hydration", hydration);
        let _ = state.status.set("hp", 100);
        state.is_alive = alive;
        state
    }
}
