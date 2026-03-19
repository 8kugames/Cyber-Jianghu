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

    /// 堆叠上限
    #[error("物品堆叠上限: {item_id}, 当前 {current}, 添加 {requested}, 最大 {max}")]
    StackLimitExceeded {
        item_id: String,
        current: i32,
        requested: i32,
        max: i32,
    },

    /// 背包已满
    #[error("背包已满")]
    InventoryFull,

    /// 数据库错误
    #[error("数据库错误: {0}")]
    DatabaseError(String),
}
