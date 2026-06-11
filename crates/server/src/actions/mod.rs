// ============================================================================
// 动作系统模块 v2 — 原子化重构
// ============================================================================
//
// 10个原子动作，每个对应唯一物理操作：
//   予/取/用    — 物品交互三原语
//   移动/说话/观察 — 空间+信息
//   攻击/休整    — 武力+时间
//   制造/教导    — 生产+知识
// ============================================================================

mod executor;
mod types;
mod validator;

pub use executor::ActionExecutor;
pub use types::{ActionExecutionResult, ItemEffect, StateChange};
pub use validator::validate_action;

use serde::{Deserialize, Serialize};

/// 予：物品从 actor 向外流动
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YuData {
    pub recipient_type: String,
    #[serde(default)]
    pub recipient_id: Option<String>,
    pub item_id: String,
    #[serde(default = "default_quantity")]
    pub quantity: i32,
}

/// 取：物品从外部流入 actor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuData {
    pub source_type: String,
    #[serde(default)]
    pub source_id: Option<String>,
    pub item_id: String,
    #[serde(default = "default_quantity")]
    pub quantity: i32,
}

/// 用：消耗或激活物品
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YongData {
    pub item_id: String,
}

/// 说话（统一通信）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeakData {
    pub content: String,
    #[serde(default = "default_channel")]
    pub channel: String,
    #[serde(default)]
    pub target_agent_id: Option<uuid::Uuid>,
}

/// 移动
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveData {
    pub target_location: String,
}

/// 观察
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObserveData {
    #[serde(default)]
    pub target_agent_id: Option<String>,
}

/// 攻击
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttackData {
    pub target_agent_id: String,
}

/// 制造
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CraftData {
    pub recipe_id: String,
}

/// 教导
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeachData {
    pub target_agent_id: String,
    pub recipe_id: String,
}

fn default_quantity() -> i32 {
    1
}
fn default_channel() -> String {
    "public".to_string()
}
