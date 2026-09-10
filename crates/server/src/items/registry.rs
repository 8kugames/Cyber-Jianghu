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
        // 通过 FromStr 解析（支持全部 5 个变体），解析失败回退到 Consumable
        let item_type = item
            .item_type
            .parse::<ItemType>()
            .unwrap_or(ItemType::Consumable);

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

/// 获取物品的稳定 uuid（UUID v5，从 item_id 确定性派生）。
///
/// 单一真源在 [`cyber_jianghu_protocol::item_uuid`]（Server/Agent 共享同一派生算法），
/// 此处转发保持 server 内 API 稳定。
pub fn item_uuid(item_id: &str) -> uuid::Uuid {
    cyber_jianghu_protocol::item_uuid(item_id)
}

/// 物品展示名：名称[短 uuid 前 8 位]。
///
/// 与角色展示名（姓名[短 uuid]）同构：既可读（名称），又可还原（短 uuid）。
/// 未注册物品理论上不可达（入口全量校验）；
/// 一旦出现（配置漂移/LLM 幻觉穿透），大声标注而非静默退化。
pub fn display_item_name(item_id: &str) -> String {
    match get_item_definition(item_id) {
        Some(def) => format!("{}[{}]", def.name, &item_uuid(item_id).to_string()[..8]),
        None => format!("未知物品[{}]", &item_uuid(item_id).to_string()[..8]),
    }
}

/// 从物品 uuid 反查内部 item_id（uuid → item_id，严格模式）。
///
/// v5 为单向派生，反查靠枚举注册表（items.yaml 数量有限，开销可忽略）。
/// 仅接受完整 uuid；裸 item_id 一律返回 None（强制全链路 uuid 引用）。
pub fn resolve_item_id(uuid_str: &str) -> Option<String> {
    let target = uuid::Uuid::parse_str(uuid_str).ok()?;
    crate::game_data::registry::ItemRegistry::all_item_ids()
        .iter()
        .find(|id| item_uuid(id) == target)
        .cloned()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_item_uuid_deterministic() {
        let a = item_uuid("mantou");
        let b = item_uuid("mantou");
        assert_eq!(a, b, "同一 item_id 必须派生出同一 uuid");
        assert_ne!(a, item_uuid("knife"), "不同 item_id 必须派生出不同 uuid");
        assert_eq!(a.get_version_num(), 5, "必须是 v5 uuid");
    }
}
