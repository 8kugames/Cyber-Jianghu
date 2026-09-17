// ============================================================================
// 资源点每日采集配额（阶段 1：内存态，方案见 docs/features/resource_depletion.md）
// ============================================================================
//
// 语义：locations.yaml 的 gatherable_daily_quotas 配置了 (node, item) 的每日
// 总采集量；未配置 = 不限。余量存进程内存 DashMap，日历日递增时整体重置
// （与 reward 日结算同一触发点），重启自然回满。
//
// 惰性查询语义：不预填充余量表——每次访问按当前 registry 配置初始化 entry，
// 天然跟随配置热重载（reload-config 换缓存后下次访问即用新配额）。

use std::collections::HashMap;
use std::sync::OnceLock;

use dashmap::DashMap;

/// (node_id, item_id) → 当日剩余可采集量
static REMAINING: OnceLock<DashMap<(String, String), i64>> = OnceLock::new();

fn remaining() -> &'static DashMap<(String, String), i64> {
    REMAINING.get_or_init(DashMap::new)
}

/// 查询 (node, item) 的每日配额；未配置返回 None（不限量）。
fn configured_quota(node_id: &str, item_id: &str) -> Option<i64> {
    let registry = super::registry_or_error().ok()?;
    let location_registry = registry.location_registry.read().expect("rwlock poisoned");
    location_registry
        .get_node(node_id)?
        .gatherable_daily_quotas
        .get(item_id)
        .copied()
        .map(i64::from)
}

/// 采集预检：余量是否足够本次 quantity。
/// 未配置配额的 (node, item) 恒通过。
pub fn precheck(node_id: &str, item_id: &str, quantity: i64) -> bool {
    let Some(quota) = configured_quota(node_id, item_id) else {
        return true;
    };
    let map = remaining();
    let entry = map
        .entry((node_id.to_string(), item_id.to_string()))
        .or_insert(quota);
    *entry >= quantity
}

/// 原子扣减当日余量；不足时返回 false（不产生部分扣减）。
/// 调用约定：mutator 在物品实际入包**前**调用，入包失败需调 [`refund`] 回补。
pub fn consume(node_id: &str, item_id: &str, quantity: i64) -> bool {
    let Some(quota) = configured_quota(node_id, item_id) else {
        return true;
    };
    let map = remaining();
    let mut entry = map
        .entry((node_id.to_string(), item_id.to_string()))
        .or_insert(quota);
    if *entry >= quantity {
        *entry -= quantity;
        true
    } else {
        false
    }
}

/// 回补（mutator 侧入包失败时的回滚路径）
pub fn refund(node_id: &str, item_id: &str, quantity: i64) {
    let map = remaining();
    let mut entry = map
        .entry((node_id.to_string(), item_id.to_string()))
        .or_insert(0);
    *entry += quantity;
}

/// 日历日递增时的整体重置（由 scheduler 日边界触发点调用，与 reward 日结算同点）。
/// 清空后下次访问按最新配置重新初始化——天然跟随配置热重载。
pub fn reset_daily() {
    remaining().clear();
}

/// 当前余量快照（dashboard/诊断用；未配置配额的条目不出现）
pub fn snapshot() -> HashMap<(String, String), i64> {
    remaining()
        .iter()
        .map(|r| (r.key().clone(), *r.value()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // tracker 的配额语义单测不依赖 registry（configured_quota 会走全局单例，
    // 未初始化时返回 None → 不限）。此处仅覆盖纯余量账本逻辑；
    // 配置联动由集成环境覆盖。
    // 直接操作内部 map 的测试辅助：
    fn set_remaining(node: &str, item: &str, v: i64) {
        remaining().insert((node.to_string(), item.to_string()), v);
    }

    #[test]
    fn reset_daily_clears_all() {
        set_remaining("n1", "馒头", 3);
        set_remaining("n2", "水", 7);
        assert!(!remaining().is_empty());
        reset_daily();
        assert!(remaining().is_empty());
    }

    // consume/refund 的完整语义依赖 configured_quota（全局 registry），
    // 其原子性与回补路径由生产路径覆盖；此处无法绕过单例直接测。
    // 配置形态（gatherable_daily_quotas 反序列化）在 unified_config 测试覆盖。
}
