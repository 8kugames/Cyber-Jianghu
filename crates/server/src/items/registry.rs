use std::collections::HashMap;
use std::sync::RwLock;

use crate::game_data::ItemConfigEntry;
use crate::models::ItemType;
use cyber_jianghu_protocol::GameError;

use super::types::ItemDefinition;

// ============================================================================
// 物品定义缓存（数据驱动）
// ============================================================================

/// 物品定义缓存
/// 使用 RwLock 包装 HashMap，使得测试时能够重置状态
static ITEM_CACHE: std::sync::LazyLock<RwLock<HashMap<String, ItemDefinition>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

static CACHE_INITIALIZED: std::sync::LazyLock<RwLock<bool>> =
    std::sync::LazyLock::new(|| RwLock::new(false));

/// 从配置初始化物品缓存
///
/// 使用配置文件中的物品定义初始化缓存
/// 必须在服务器启动时调用，之后 get_item_definition 才能正常工作
///
/// # 参数
/// - `config_items`: 配置的物品列表
///
/// # 返回
/// - Ok(()): 初始化成功
/// - Err(GameError): 缓存已初始化（不允许重复初始化）
///
/// # 注意
/// - 只能在缓存未初始化时调用
/// - 如果缓存已初始化，返回错误
/// - 如果需要重新加载物品定义，需要重启服务器
pub fn init_item_cache_from_config(config_items: &[ItemConfigEntry]) -> Result<(), GameError> {
    // 处理可能被污染的锁
    let mut initialized = match CACHE_INITIALIZED.write() {
        Ok(guard) => guard,
        Err(e) => e.into_inner(),
    };

    if *initialized {
        return Err(GameError::Validation(
            "Item cache already initialized".to_string(),
        ));
    }

    let mut cache = match ITEM_CACHE.write() {
        Ok(guard) => guard,
        Err(e) => e.into_inner(),
    };

    for item in config_items {
        let item_type = match item.item_type.as_str() {
            "consumable" => ItemType::Consumable,
            "currency" => ItemType::Currency,
            "weapon" => ItemType::Weapon,
            _ => ItemType::Consumable, // 默认值
        };

        let def = ItemDefinition::new(
            &item.item_id,
            &item.name,
            item_type,
            item.effects.clone(),
            &item.description,
            item.max_durability,
            item.decay_rate,
        );

        cache.insert(item.item_id.clone(), def);
    }

    *initialized = true;
    Ok(())
}

/// 检查物品缓存是否已初始化
#[allow(dead_code)]
pub fn is_item_cache_initialized() -> bool {
    match CACHE_INITIALIZED.read() {
        Ok(guard) => *guard,
        Err(_) => false, // 锁被污染，视为未初始化
    }
}

/// 获取物品定义
///
/// 根据物品ID获取物品定义（使用缓存）
///
/// # 返回
/// - Some(item): 物品存在
/// - None: 物品不存在或缓存未初始化
///
/// # 注意
/// 必须先调用 init_item_cache_from_config() 初始化缓存
pub fn get_item_definition(item_id: &str) -> Option<ItemDefinition> {
    match ITEM_CACHE.read() {
        Ok(guard) => guard.get(item_id).cloned(),
        Err(_) => None, // 锁被污染，返回None
    }
}

/// 获取货币物品 ID（数据驱动）
///
/// 从缓存中查找 item_type 为 Currency 的物品，返回其 item_id。
/// 如果存在多个货币，返回第一个；如果没有货币，回退到 "银子"。
pub fn get_currency_item_id() -> String {
    match ITEM_CACHE.read() {
        Ok(guard) => {
            for (id, def) in guard.iter() {
                if def.item_type == ItemType::Currency {
                    return id.clone();
                }
            }
            // 回退：缓存中没有 Currency 类型
            "银子".to_string()
        }
        Err(_) => "银子".to_string(),
    }
}

// 仅用于测试的重置函数
#[cfg(test)]
pub(crate) fn reset_item_cache() {
    // 处理可能被污染的锁（当测试panic时）
    let mut cache = match ITEM_CACHE.write() {
        Ok(guard) => guard,
        Err(e) => {
            // 锁被污染，恢复并获取写入权限
            e.into_inner()
        }
    };
    cache.clear();

    let mut init_flag = match CACHE_INITIALIZED.write() {
        Ok(guard) => guard,
        Err(e) => e.into_inner(),
    };
    *init_flag = false;
}
