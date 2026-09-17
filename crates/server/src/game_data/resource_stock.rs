// ============================================================================
// 资源点存量（阶段 2）：DB 持久权威 + 内存缓存
// ============================================================================
//
// 架构（docs/features/resource_depletion.md 阶段 2）：
// - 权威：PostgreSQL resource_nodes 表（采集在 Saga 事务内扣减，回滚自动恢复）
// - 读缓存：内存 DashMap（启动/日再生后从 DB 加载），渲染与预检零 DB 负载
// - 写穿透：mutator 扣减同时更新 DB（tx 内）与内存
// - 日再生：scheduler 日历日递增触发点按 regen_per_game_day 回补（受四季
//   resource_growth_rate 加成，上限 max_stock），DB 与内存同步
//
// 配置声明在 locations.yaml 的 gatherable_stocks（max_stock/init_ratio/
// regen_per_game_day）；未配置的 (node, item) 不限（无限采集）。

use std::collections::HashMap;
use std::sync::OnceLock;

use anyhow::Context;
use dashmap::DashMap;
use sqlx::PgPool;

/// (node_id, item_id) → 当前存量。只包含配置了 gatherable_stocks 的条目。
static STOCKS: OnceLock<DashMap<(String, String), i64>> = OnceLock::new();

fn stocks() -> &'static DashMap<(String, String), i64> {
    STOCKS.get_or_init(DashMap::new)
}

/// 某节点全部资源存量快照（WorldState 渲染用；未配置的 item 不出现）
pub fn node_stocks(node_id: &str) -> HashMap<String, i64> {
    stocks()
        .iter()
        .filter(|r| r.key().0 == node_id)
        .map(|r| (r.key().1.clone(), *r.value()))
        .collect()
}

/// 单项存量（渲染/诊断用；None = 未配置存量模型）
pub fn stock_of(node_id: &str, item_id: &str) -> Option<i64> {
    stocks()
        .get(&(node_id.to_string(), item_id.to_string()))
        .map(|r| *r)
}

/// 启动初始化：upsert yaml 声明的存量（已有行保留现值——重启不回满），
/// 然后全量加载进内存缓存。registry 必须已初始化（读 gatherable_stocks 配置）。
pub async fn init_from_db(pool: &PgPool) -> anyhow::Result<()> {
    let registry = super::registry_or_error()
        .map_err(|e| anyhow::anyhow!("resource_stock::init_from_db: 注册表未初始化: {e}"))?;
    // 读锁不跨 await：先把声明性配置克隆出来
    let declarations: Vec<(String, String, i64, i64, i64)> = {
        let location_registry = registry.location_registry.read().expect("rwlock poisoned");
        location_registry
            .graph_nodes()
            .flat_map(|node| {
                node.gatherable_stocks.iter().map(move |(item_id, cfg)| {
                    let init = (cfg.max_stock as f32 * cfg.init_ratio).round() as i64;
                    (
                        node.node_id.clone(),
                        item_id.clone(),
                        init,
                        i64::from(cfg.max_stock),
                        i64::from(cfg.regen_per_game_day),
                    )
                })
            })
            .collect()
    };

    // 1) upsert 声明性配置（缺行才插；重启/热重载不重置已有存量）
    for (node_id, item_id, init, max_stock, regen) in &declarations {
        sqlx::query(
            "INSERT INTO resource_nodes (node_id, item_id, stock, max_stock, regen_per_game_day) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (node_id, item_id) DO NOTHING",
        )
        .bind(node_id)
        .bind(item_id)
        .bind(init)
        .bind(max_stock)
        .bind(regen)
        .execute(pool)
        .await
        .context("resource_nodes upsert 初始配置失败")?;
    }

    // 2) 全量加载进内存
    let rows: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT node_id, item_id, stock FROM resource_nodes")
            .fetch_all(pool)
            .await
            .context("resource_nodes 加载失败")?;
    let map = stocks();
    for (node_id, item_id, stock) in rows {
        map.insert((node_id, item_id), stock);
    }
    tracing::info!("[resource_stock] 已加载 {} 条资源存量", map.len());
    Ok(())
}

/// 采集预检（executor 快速失败路径）：内存读，配置了存量的条目余量须足够。
pub fn precheck(node_id: &str, item_id: &str, quantity: i64) -> bool {
    match stock_of(node_id, item_id) {
        None => true, // 未配置存量模型 = 不限
        Some(stock) => stock >= quantity,
    }
}

/// 采集扣减（mutator，Saga 事务内）：DB 权威扣减（stock >= quantity 守卫，
/// 不足返回 false 即枯竭），成功后同步内存。事务回滚时 DB 扣减自动恢复。
pub async fn consume_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    node_id: &str,
    item_id: &str,
    quantity: i64,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "UPDATE resource_nodes SET stock = stock - $3, updated_at = NOW() \
         WHERE node_id = $1 AND item_id = $2 AND stock >= $3",
    )
    .bind(node_id)
    .bind(item_id)
    .bind(quantity)
    .execute(&mut **tx)
    .await
    .context("resource_nodes 扣减失败")?;
    let ok = result.rows_affected() == 1;
    if ok && let Some(mut entry) = stocks().get_mut(&(node_id.to_string(), item_id.to_string())) {
        *entry -= quantity;
    }
    Ok(ok)
}

/// 日再生（scheduler 日历日递增触发点）：按四季 resource_growth_rate 加成回补，
/// 上限 max_stock；随后重载内存。失败仅告警不阻断 tick（调用方处理）。
pub async fn regen_daily(pool: &PgPool, season_growth_rate: f32) -> anyhow::Result<u64> {
    let result = sqlx::query(
        "UPDATE resource_nodes \
         SET stock = LEAST(stock + CEIL(regen_per_game_day * $1)::bigint, max_stock), \
             updated_at = NOW() \
         WHERE regen_per_game_day > 0 AND stock < max_stock",
    )
    .bind(season_growth_rate as f64)
    .execute(pool)
    .await
    .context("resource_nodes 日再生失败")?;

    // 重载内存（行数少，全量代价可忽略）
    let rows: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT node_id, item_id, stock FROM resource_nodes")
            .fetch_all(pool)
            .await?;
    let map = stocks();
    for (node_id, item_id, stock) in rows {
        map.insert((node_id, item_id), stock);
    }
    Ok(result.rows_affected())
}
