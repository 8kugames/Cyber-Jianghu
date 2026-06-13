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
mod schema_validator;
mod types;
mod validator;

pub use executor::ActionExecutor;
pub use schema_validator::{SchemaViolation, ViolationType, validate_action_data_schema};
pub use types::{ActionExecutionResult, ItemEffect, StateChange};
pub use validator::validate_action;

use cyber_jianghu_protocol::GameError;
use serde::de::DeserializeOwned;
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

/// 类型安全的动作数据桥接 enum
///
/// 验证层直接反序列化 `intent.action_data` 到对应 typed struct，
/// 执行层直接匹配 enum variant 获取 typed data，消除双重解析和 Value 字段访问。
#[derive(Debug, Clone)]
pub enum ParsedActionData {
    Yu(YuData),
    Qu(QuData),
    Yong(YongData),
    Speak(SpeakData),
    Move(MoveData),
    Observe(ObserveData),
    Attack(AttackData),
    Craft(CraftData),
    Teach(TeachData),
    /// 无参数动作（如休整）
    None,
}

/// 从可选的 JSON Value 反序列化为指定类型
pub fn parse_action_data<T: DeserializeOwned>(
    action_data: &Option<serde_json::Value>,
    action_name: &str,
) -> Result<T, GameError> {
    match action_data {
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| GameError::InvalidActionData {
            reason: format!("{} 参数解析错误: {}", action_name, e),
        }),
        None => Err(GameError::InvalidActionData {
            reason: format!("{} 缺少 action_data", action_name),
        }),
    }
}

impl ParsedActionData {
    /// 获取字符串字段值（用于 field_validations 在 typed 数据上执行）
    pub fn get_field_str(&self, field: &str) -> Option<String> {
        match (self, field) {
            (Self::Yu(d), "recipient_type") => Some(d.recipient_type.clone()),
            (Self::Yu(d), "item_id") => Some(d.item_id.clone()),
            (Self::Yu(d), "recipient_id") => d.recipient_id.clone(),
            (Self::Qu(d), "source_type") => Some(d.source_type.clone()),
            (Self::Qu(d), "item_id") => Some(d.item_id.clone()),
            (Self::Qu(d), "source_id") => d.source_id.clone(),
            (Self::Yong(d), "item_id") => Some(d.item_id.clone()),
            (Self::Speak(d), "content") => Some(d.content.clone()),
            (Self::Speak(d), "channel") => Some(d.channel.clone()),
            (Self::Move(d), "target_location") => Some(d.target_location.clone()),
            (Self::Craft(d), "recipe_id") => Some(d.recipe_id.clone()),
            (Self::Teach(d), "recipe_id") => Some(d.recipe_id.clone()),
            (Self::Teach(d), "target_agent_id") => Some(d.target_agent_id.clone()),
            (Self::Attack(d), "target_agent_id") => Some(d.target_agent_id.clone()),
            (Self::Observe(d), "target_agent_id") => d.target_agent_id.clone(),
            _ => None,
        }
    }

    /// 获取整数字段值（用于 field_validations 在 typed 数据上执行）
    pub fn get_field_i32(&self, field: &str) -> Option<i32> {
        match (self, field) {
            (Self::Yu(d), "quantity") => Some(d.quantity),
            (Self::Qu(d), "quantity") => Some(d.quantity),
            _ => None,
        }
    }

    /// 从 typed 数据中提取 target_agent_id（统一处理 String/Option<String>/Option<Uuid>）
    pub fn get_target_agent_id(&self) -> Option<String> {
        match self {
            Self::Attack(d) => Some(d.target_agent_id.clone()),
            Self::Teach(d) => Some(d.target_agent_id.clone()),
            Self::Speak(d) => d.target_agent_id.map(|id| id.to_string()),
            Self::Observe(d) => d.target_agent_id.clone(),
            _ => None,
        }
    }
}

fn default_quantity() -> i32 {
    1
}
fn default_channel() -> String {
    "public".to_string()
}
