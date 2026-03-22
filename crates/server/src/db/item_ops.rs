// ============================================================================
// OpenClaw Cyber-Jianghu MVP 物品数据库操作模块
// ============================================================================
//
// 本模块实现物品相关的数据库操作，包括：
// - 从配置同步物品到数据库（用于外键约束）

use anyhow::{Context, Result};
use serde_json;
use sqlx::PgPool;
use tracing::{debug, info};

use crate::game_data::types::ItemConfigEntry;

/// 从配置同步物品到数据库
///
/// 使用 UPSERT 确保物品表中有对应的记录（用于外键约束）
///
/// # 参数
/// - pool: 数据库连接池
/// - items: 物品配置列表
///
/// # 返回
/// - Ok(usize): 同步的物品数量
/// - Err: 数据库操作失败
pub async fn sync_items_from_config(pool: &PgPool, items: &[ItemConfigEntry]) -> Result<usize> {
    debug!("开始同步物品到数据库，共 {} 种", items.len());

    let mut synced = 0;
    for item in items {
        let effects_json = serde_json::to_value(&item.effects).context("序列化物品效果失败")?;

        let result = sqlx::query(
            r#"
            INSERT INTO items (item_id, name, item_type, effects, stack_size, description)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (item_id) DO UPDATE SET
                name = EXCLUDED.name,
                item_type = EXCLUDED.item_type,
                effects = EXCLUDED.effects,
                stack_size = EXCLUDED.stack_size,
                description = EXCLUDED.description
            "#,
        )
        .bind(&item.item_id)
        .bind(&item.name)
        .bind(&item.item_type)
        .bind(&effects_json)
        .bind(item.stack_size)
        .bind(&item.description)
        .execute(pool)
        .await
        .context(format!("同步物品 {} 失败", item.item_id))?;

        if result.rows_affected() > 0 {
            synced += 1;
        }
    }

    info!("物品同步完成，共 {} 种", items.len());
    Ok(synced)
}
