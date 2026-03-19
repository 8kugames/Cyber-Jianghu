//! sqlx 数据库类型支持
//!
//! 仅在服务端使用，需要启用 `sqlx-support` feature

use serde::{Deserialize, Serialize};
use sqlx::{Encode, Postgres, Type, postgres::PgArgumentBuffer};
use std::fmt;
use std::str::FromStr;

// 重导出 ActionType，因为我们会添加 sqlx 支持
pub use crate::types::ActionType;

/// 为 ActionType 实现 `sqlx::Type<Postgres>`
impl Type<Postgres> for ActionType {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        sqlx::postgres::PgTypeInfo::with_name("text")
    }
}

/// 为 ActionType 实现 `sqlx::Encode<Postgres>`
impl Encode<'_, Postgres> for ActionType {
    fn encode_by_ref(
        &self,
        buf: &mut PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        let s = self.to_string();
        <String as Encode<Postgres>>::encode_by_ref(&s, buf)
    }
}

/// 为 ActionType 实现 `sqlx::Decode<Postgres>`
impl sqlx::Decode<'_, Postgres> for ActionType {
    fn decode(value: sqlx::postgres::PgValueRef<'_>) -> Result<Self, sqlx::error::BoxDynError> {
        let s = <String as sqlx::Decode<Postgres>>::decode(value)?;
        Ok(ActionType::new(s))
    }
}

/// 动作执行结果（带 sqlx 支持）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "text", rename_all = "lowercase")]
pub enum ActionResult {
    /// 成功
    Success,

    /// 失败
    Failed,
}

impl fmt::Display for ActionResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

impl FromStr for ActionResult {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "success" => Ok(Self::Success),
            "failed" => Ok(Self::Failed),
            _ => Err(format!("Invalid action result: {}", s)),
        }
    }
}

/// Tick 执行状态（带 sqlx 支持）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "text", rename_all = "lowercase")]
pub enum TickStatus {
    /// 运行中
    Running,

    /// 已完成
    Completed,

    /// 失败
    Failed,
}

impl fmt::Display for TickStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

impl FromStr for TickStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            _ => Err(format!("Invalid tick status: {}", s)),
        }
    }
}

/// 物品类型（带 sqlx 支持）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "text", rename_all = "lowercase")]
pub enum ItemType {
    /// 消耗品（如馒头、水）
    Consumable,

    /// 武器（如刀）
    Weapon,

    /// 货币（如银子）
    Currency,
}

impl fmt::Display for ItemType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Consumable => write!(f, "consumable"),
            Self::Weapon => write!(f, "weapon"),
            Self::Currency => write!(f, "currency"),
        }
    }
}

impl FromStr for ItemType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "consumable" => Ok(Self::Consumable),
            "weapon" => Ok(Self::Weapon),
            "currency" => Ok(Self::Currency),
            _ => Err(format!("Invalid item type: {}", s)),
        }
    }
}

/// 物品效果类型（带 sqlx 支持）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum EffectType {
    /// 恢复饥饿值
    RestoreHunger,

    /// 恢复口渴值
    RestoreThirst,

    /// 增加攻击力
    IncreaseAttack,
}

impl fmt::Display for EffectType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RestoreHunger => write!(f, "restore_hunger"),
            Self::RestoreThirst => write!(f, "restore_thirst"),
            Self::IncreaseAttack => write!(f, "increase_attack"),
        }
    }
}

impl FromStr for EffectType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "restore_hunger" => Ok(Self::RestoreHunger),
            "restore_thirst" => Ok(Self::RestoreThirst),
            "increase_attack" => Ok(Self::IncreaseAttack),
            _ => Err(format!("Invalid effect type: {}", s)),
        }
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_result_display() {
        assert_eq!(ActionResult::Success.to_string(), "success");
        assert_eq!(ActionResult::Failed.to_string(), "failed");
    }

    #[test]
    fn test_action_result_from_str() {
        assert_eq!(
            ActionResult::from_str("success").unwrap(),
            ActionResult::Success
        );
        assert_eq!(
            ActionResult::from_str("FAILED").unwrap(),
            ActionResult::Failed
        );
    }

    #[test]
    fn test_tick_status_display() {
        assert_eq!(TickStatus::Running.to_string(), "running");
        assert_eq!(TickStatus::Completed.to_string(), "completed");
    }

    #[test]
    fn test_item_type_display() {
        assert_eq!(ItemType::Consumable.to_string(), "consumable");
        assert_eq!(ItemType::Weapon.to_string(), "weapon");
    }
}
