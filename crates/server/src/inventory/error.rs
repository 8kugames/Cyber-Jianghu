// ============================================================================
// OpenClaw Cyber-Jianghu 背包错误
// ============================================================================

use thiserror::Error;

/// 背包操作错误
#[derive(Debug, Error)]
pub enum InventoryError {
    /// 物品不存在
    #[error("物品不存在: {0}")]
    ItemNotFound(String),

    /// 物品数量不足
    #[error("物品数量不足: 需要 {required}, 拥有 {available}")]
    InsufficientQuantity { required: i32, available: i32 },

    /// 背包已满
    #[error("背包已满")]
    InventoryFull,

    /// 数据库错误
    #[error("数据库错误: {0}")]
    DatabaseError(String),
}
