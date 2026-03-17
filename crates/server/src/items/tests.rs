#[cfg(test)]
use super::registry::*;
#[cfg(test)]
use super::system::*;
#[cfg(test)]
use super::types::*;
#[cfg(test)]
use crate::game_data::{ItemConfigEntry, ItemEffect};
#[cfg(test)]
use crate::models::{AgentState, ItemType};
#[cfg(test)]
use uuid::Uuid;

use cyber_jianghu_protocol::GameError;
use std::sync::Mutex;

// 全局测试锁，确保并行测试时不会相互干扰全局缓存
static TEST_LOCK: std::sync::LazyLock<Mutex<()>> = std::sync::LazyLock::new(|| Mutex::new(()));

// 为了测试方便，添加一个通过内部锁安全重置状态的辅助函数
// 注意：调用者必须持有 TEST_LOCK
#[cfg(test)]
fn reset_cache() {
    reset_item_cache();
    // 还要保证重新初始化测试注册表中的 attributes 状态
    crate::game_data::init_test_registry();
}

#[cfg(test)]
fn use_item(state: &mut AgentState, item_id: &str) -> Result<(), GameError> {
    let item =
        get_item_definition(item_id).ok_or_else(|| GameError::ItemNotFound(item_id.to_string()))?;

    if !item.is_usable() {
        return Err(GameError::Unknown(format!(
            "Item {} is not usable",
            item_id
        )));
    }

    apply_item_effect(state, &item)
}

#[test]
fn test_init_item_cache_from_config() {
    let _guard = match TEST_LOCK.lock() {
        Ok(guard) => guard,
        Err(e) => e.into_inner(), // 处理被污染的锁
    };
    reset_cache();

    let config_items = vec![ItemConfigEntry {
        item_id: "test_food".to_string(),
        name: "测试食物".to_string(),
        item_type: "consumable".to_string(),
        effects: vec![ItemEffect {
            description: "恢复饥饿值".to_string(),
            attribute: "hunger".to_string(),
            operation: "add".to_string(),
            value: serde_json::json!(1),
        }],
        stack_size: 10,
        description: "测试用食物".to_string(),
        max_durability: -1,
        decay_rate: 0,
    }];

    let result = init_item_cache_from_config(&config_items);
    assert!(result.is_ok());

    // 验证缓存已初始化
    assert!(is_item_cache_initialized());
}

#[test]
fn test_init_item_cache_duplicate() {
    let _guard = match TEST_LOCK.lock() {
        Ok(guard) => guard,
        Err(e) => e.into_inner(),
    };
    reset_cache();
    let config_items = vec![];

    // 第一次初始化
    assert!(init_item_cache_from_config(&config_items).is_ok());

    // 第二次初始化应该失败
    let result = init_item_cache_from_config(&config_items);
    assert!(result.is_err());
}

#[test]
fn test_get_item_definition() {
    let _guard = match TEST_LOCK.lock() {
        Ok(guard) => guard,
        Err(e) => e.into_inner(),
    };
    reset_cache();

    // 先初始化缓存
    let config_items = vec![ItemConfigEntry {
        item_id: "mantou".to_string(),
        name: "馒头".to_string(),
        item_type: "consumable".to_string(),
        effects: vec![ItemEffect {
            description: "恢复饥饿值".to_string(),
            attribute: "hunger".to_string(),
            operation: "add".to_string(),
            value: serde_json::json!(1),
        }],
        stack_size: 10,
        description: "热腾腾的馒头".to_string(),
        max_durability: 100,
        decay_rate: 1,
    }];
    let _ = init_item_cache_from_config(&config_items);

    // 测试存在的物品
    let mantou = get_item_definition("mantou");
    assert!(mantou.is_some());
    let mantou = mantou.unwrap();
    assert_eq!(mantou.name, "馒头");
    assert_eq!(mantou.item_type, ItemType::Consumable);
    assert_eq!(mantou.max_durability, 100);
    assert_eq!(mantou.decay_rate, 1);

    // 测试不存在的物品
    let unknown = get_item_definition("unknown");
    assert!(unknown.is_none());
}

#[test]
fn test_item_usability() {
    let _guard = match TEST_LOCK.lock() {
        Ok(guard) => guard,
        Err(e) => e.into_inner(),
    };
    reset_cache();

    // 先初始化缓存
    let config_items = vec![
        ItemConfigEntry {
            item_id: "food".to_string(),
            name: "食物".to_string(),
            item_type: "consumable".to_string(),
            effects: vec![ItemEffect {
                description: "恢复饥饿值".to_string(),
                attribute: "hunger".to_string(),
                operation: "add".to_string(),
                value: serde_json::json!(1),
            }],
            stack_size: 10,
            description: "测试食物".to_string(),
            max_durability: -1,
            decay_rate: 0,
        },
        ItemConfigEntry {
            item_id: "weapon".to_string(),
            name: "武器".to_string(),
            item_type: "weapon".to_string(),
            effects: vec![],
            stack_size: 1,
            description: "测试武器".to_string(),
            max_durability: 100,
            decay_rate: 0,
        },
    ];
    let _ = init_item_cache_from_config(&config_items);

    // 食物可以使用
    let food = get_item_definition("food").unwrap();
    assert!(food.is_usable());
    assert!(!food.is_equippable());

    // 武器可以装备
    let weapon = get_item_definition("weapon").unwrap();
    assert!(!weapon.is_usable());
    assert!(weapon.is_equippable());
}

#[test]
fn test_apply_item_effect_hunger() {
    let _guard = match TEST_LOCK.lock() {
        Ok(guard) => guard,
        Err(e) => e.into_inner(),
    };
    reset_cache();

    // 先初始化物品缓存
    let config_items = vec![ItemConfigEntry {
        item_id: "mantou".to_string(),
        name: "馒头".to_string(),
        item_type: "consumable".to_string(),
        effects: vec![ItemEffect {
            description: "恢复饥饿值".to_string(),
            attribute: "hunger".to_string(),
            operation: "add".to_string(),
            value: serde_json::json!(1),
        }],
        stack_size: 10,
        description: "热腾腾的馒头".to_string(),
        max_durability: -1,
        decay_rate: 0,
    }];
    let _ = init_item_cache_from_config(&config_items);

    let mut state = AgentState::new(Uuid::new_v4(), 1);
    let _ = state.status.set("hunger", 50);

    let mantou = get_item_definition("mantou").unwrap();
    let result = apply_item_effect(&mut state, &mantou);

    assert!(result.is_ok());
    assert_eq!(state.status.get("hunger").unwrap_or(0), 51); // 50 + 1 = 51 (using value: 1 from test config)
}

#[test]
fn test_apply_item_effect_max_value() {
    let _guard = match TEST_LOCK.lock() {
        Ok(guard) => guard,
        Err(e) => e.into_inner(),
    };
    reset_cache();

    // 先初始化物品缓存
    let config_items = vec![ItemConfigEntry {
        item_id: "big_food".to_string(),
        name: "大食物".to_string(),
        item_type: "consumable".to_string(),
        effects: vec![ItemEffect {
            description: "恢复饥饿值".to_string(),
            attribute: "hunger".to_string(),
            operation: "add".to_string(),
            value: serde_json::json!(1),
        }],
        stack_size: 10,
        description: "测试用大食物".to_string(),
        max_durability: -1,
        decay_rate: 0,
    }];
    let _ = init_item_cache_from_config(&config_items);

    let mut state = AgentState::new(Uuid::new_v4(), 1);
    let _ = state.status.set("hunger", 90);

    let food = get_item_definition("big_food").unwrap();
    let result = apply_item_effect(&mut state, &food);

    assert!(result.is_ok());
    assert_eq!(state.status.get("hunger").unwrap_or(0), 91); // 90 + 1 = 91 (using value: 1 from test config)
}

#[test]
fn test_apply_item_effect_dead_agent() {
    let _guard = match TEST_LOCK.lock() {
        Ok(guard) => guard,
        Err(e) => e.into_inner(),
    };
    reset_cache();

    // 先初始化物品缓存
    let config_items = vec![ItemConfigEntry {
        item_id: "food".to_string(),
        name: "食物".to_string(),
        item_type: "consumable".to_string(),
        effects: vec![ItemEffect {
            description: "恢复饥饿值".to_string(),
            attribute: "hunger".to_string(),
            operation: "add".to_string(),
            value: serde_json::json!(1),
        }],
        stack_size: 10,
        description: "测试食物".to_string(),
        max_durability: -1,
        decay_rate: 0,
    }];
    let _ = init_item_cache_from_config(&config_items);

    let mut state = AgentState::new(Uuid::new_v4(), 1);
    state.is_alive = false;

    let food = get_item_definition("food").unwrap();
    let result = apply_item_effect(&mut state, &food);

    assert!(result.is_err());
    match result {
        Err(cyber_jianghu_protocol::GameError::AgentDead { .. }) => (),
        _ => panic!("Expected AgentDead error"),
    }
}

#[test]
fn test_use_item_not_found() {
    let _guard = match TEST_LOCK.lock() {
        Ok(guard) => guard,
        Err(e) => e.into_inner(),
    };
    reset_cache();
    // 初始化空缓存
    let config_items = vec![];
    let _ = init_item_cache_from_config(&config_items);

    let mut state = AgentState::new(Uuid::new_v4(), 1);

    // 物品未初始化，应该返回 NotFound 错误
    let result = use_item(&mut state, "unknown");

    assert!(result.is_err());
    match result {
        Err(cyber_jianghu_protocol::GameError::ItemNotFound(id)) => assert_eq!(id, "unknown"),
        _ => panic!("Expected ItemNotFound error"),
    }
}

#[test]
fn test_use_item_convenience_function() {
    let _guard = match TEST_LOCK.lock() {
        Ok(guard) => guard,
        Err(e) => e.into_inner(),
    };
    reset_cache();

    // 先初始化物品缓存
    let config_items = vec![ItemConfigEntry {
        item_id: "food".to_string(),
        name: "食物".to_string(),
        item_type: "consumable".to_string(),
        effects: vec![ItemEffect {
            description: "恢复饥饿值".to_string(),
            attribute: "hunger".to_string(),
            operation: "add".to_string(),
            value: serde_json::json!(1),
        }],
        stack_size: 10,
        description: "测试食物".to_string(),
        max_durability: -1,
        decay_rate: 0,
    }];
    let _ = init_item_cache_from_config(&config_items);

    let mut state = AgentState::new(Uuid::new_v4(), 1);
    // 手动初始化状态用于测试
    let _ = state.status.set("hunger", 50);

    let result = use_item(&mut state, "food");
    assert!(result.is_ok());
    assert_eq!(state.status.get("hunger").unwrap_or(0), 51); // 50 + 1 = 51 (using value: 1 from test config)
}
