#[cfg(test)]
use super::registry::*;
#[cfg(test)]
use crate::game_data::{ItemConfigEntry, ItemEffect};
#[cfg(test)]
use crate::models::ItemType;

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
        item_id: "馒头".to_string(),
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
    let mantou = get_item_definition("馒头");
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

    // 武器可以使用（会有负向效果）且可装备
    let weapon = get_item_definition("weapon").unwrap();
    assert!(weapon.is_usable());
    assert!(weapon.is_equippable());
}
