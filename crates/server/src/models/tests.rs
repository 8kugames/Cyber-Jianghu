// ============================================================================
// OpenClaw Cyber-Jianghu 测试
// ============================================================================
//
// 本模块从 models/mod.rs 拆分出来，包含所有测试函数
// ============================================================================

use super::tick::TickStatus;
use super::*;
use uuid::Uuid;

#[test]
fn test_agent_state_decay() {
    crate::game_data::init_test_registry();

    let mut state = AgentState::new(Uuid::new_v4(), 1);

    // 白板重生初始值：HP=100, 体力=100, 饥饿=50, 口渴=50
    assert_eq!(state.status.get("satiation").unwrap_or(0), 50);
    assert_eq!(state.status.get("hydration").unwrap_or(0), 50);
    assert_eq!(state.status.get("hp").unwrap_or(0), 100);
    assert!(state.is_alive);

    // 测试配置：satiation/hydration decay_per_tick = 0.2
    // 单 tick 累计器未到 1.0，satiation/hydration 保持原值
    let _ = state.apply_decay(1);
    assert_eq!(state.status.get("satiation").unwrap_or(0), 50);
    assert_eq!(state.status.get("hydration").unwrap_or(0), 50);
    assert_eq!(state.status.get("stamina").unwrap_or(0), 100);

    // 跑满 5 tick，累计器到 -1.0，satiation 扣 1
    for _ in 0..4 {
        let _ = state.apply_decay(1);
    }
    assert_eq!(state.status.get("satiation").unwrap_or(0), 49);
    assert_eq!(state.status.get("hydration").unwrap_or(0), 49);

    // 验证累计器不漂移：再跑 5 tick，应再扣 1
    for _ in 0..5 {
        let _ = state.apply_decay(1);
    }
    assert_eq!(state.status.get("satiation").unwrap_or(0), 48);

    // 持续衰减至死亡：0.2/tick → 5 tick 扣 1，48 → 0 需 ~240 tick
    for _ in 0..300 {
        if !state.is_alive {
            break;
        }
        let _ = state.apply_decay(1);
    }
    // 同一 tick 内两属性同时归零时，先被处理的触发死亡并 return，另一个停在 1（遍历顺序不确定）
    assert!(state.status.get("satiation").unwrap_or(0) <= 1);
    assert!(state.status.get("hydration").unwrap_or(0) <= 1);
    assert!(!state.is_alive);
    assert_eq!(state.status.get("hp").unwrap_or(0), 0);
}

/// 验证小数 decay（如 0.2）跨 tick 累计，不被 f32→i32 截断为 0
#[test]
fn test_agent_state_decay_fractional_accumulator() {
    crate::game_data::init_test_registry();

    let mut state = AgentState::new(Uuid::new_v4(), 1);
    assert_eq!(state.status.get("satiation").unwrap_or(0), 50);

    for tick in 1..=4 {
        let _ = state.apply_decay(tick);
        assert_eq!(
            state.status.get("satiation").unwrap_or(-1),
            50,
            "tick {} 累计器未到 1.0，satiation 应保持 50",
            tick
        );
        let acc = state
            .decay_accumulator
            .get("satiation")
            .copied()
            .unwrap_or(0.0);
        assert!((acc - (-0.2 * tick as f32)).abs() < 1e-5);
    }

    let _ = state.apply_decay(5);
    assert_eq!(state.status.get("satiation").unwrap_or(-1), 49);
    let acc = state
        .decay_accumulator
        .get("satiation")
        .copied()
        .unwrap_or(0.0);
    assert!(acc.abs() < 1e-5, "累计器应在扣减后归零，实际 {}", acc);
}

#[test]
fn test_agent_state_restore() {
    crate::game_data::init_test_registry();

    let mut state = AgentState::new(Uuid::new_v4(), 1);

    // 跑满 5 tick 让 satiation/hydration 扣 1
    for _ in 0..5 {
        let _ = state.apply_decay(1);
    }
    assert_eq!(state.status.get("satiation").unwrap_or(0), 49);
    assert_eq!(state.status.get("hydration").unwrap_or(0), 49);

    // 恢复饱食度
    state.restore_attribute("satiation", 30);
    assert_eq!(state.status.get("satiation").unwrap_or(0), 79);

    // 恢复饱饮度
    state.restore_attribute("hydration", 20);
    assert_eq!(state.status.get("hydration").unwrap_or(0), 69);

    // 恢复到最大值
    state.restore_attribute("satiation", 50);
    assert_eq!(state.status.get("satiation").unwrap_or(0), 100);
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
    state.restore_attribute("satiation", 50);
    assert_eq!(state.status.get("satiation").unwrap_or(0), 50); // 死亡状态下恢复无效
}

#[test]
fn test_action_type_conversion() {
    // 数据驱动：ActionType 是字符串包装，所有字符串都有效
    let idle = ActionType::new("休整");
    assert_eq!(idle.as_str(), "休整");

    let speak = ActionType::new("说话");
    assert_eq!(speak.as_str(), "说话");

    let custom = ActionType::new("custom_action");
    assert_eq!(custom.as_str(), "custom_action");

    assert_eq!(idle.to_string(), "休整");
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

    // 验证 satiation 和 hydration 在衰减列表中
    assert!(
        decaying.iter().any(|(name, _)| name == "satiation"),
        "satiation should be in decaying attributes"
    );
    assert!(
        decaying.iter().any(|(name, _)| name == "hydration"),
        "hydration should be in decaying attributes"
    );
}
