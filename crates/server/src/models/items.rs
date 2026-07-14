// ============================================================================
// 物品相关数据结构
// ============================================================================
//
// 注意：物品定义主要来自配置文件 (items.yaml)
// 数据库 items 表用于 FK 约束，数据结构与之对应
// ============================================================================

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::game_data::Operation;

// 物品类型统一以 protocol 为权威定义（带 sqlx 支持），此处仅 re-export
pub use cyber_jianghu_protocol::sqlx_types::ItemType;

/// 物品效果（数据库模型）
///
/// 与配置文件的 ItemEffect 结构对应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemEffect {
    /// 目标属性
    pub attribute: String,

    /// 操作类型（add / set / multiply）
    #[serde(default)]
    pub operation: Operation,

    /// 效果值
    pub value: JsonValue,
}
