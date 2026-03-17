// ============================================================================
// 寿命系统类型定义
// ============================================================================
//
// 说明：寿命系统主要用于**叙事和记忆**，不影响服务端游戏逻辑。
// 年龄信息可用于生成叙事、构建记忆上下文，但不参与战斗、属性计算等服务端逻辑。
// ============================================================================

use serde::{Deserialize, Serialize};

// ============================================================================
// 老化效果
// ============================================================================

/// 老化效果（仅用于叙事，不影响服务端计算）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgingEffects {
    /// 体力上限衰减（每岁 -N）
    #[serde(default)]
    pub stamina_decay: f32,

    /// HP 上限衰减（每岁 -N）
    #[serde(default)]
    pub hp_decay: f32,

    /// 衰老起始年龄（此年龄后开始受影响）
    #[serde(default = "default_aging_start_age")]
    pub aging_start_age: u8,
}

fn default_aging_start_age() -> u8 {
    60
}

impl Default for AgingEffects {
    fn default() -> Self {
        Self {
            stamina_decay: 0.0,
            hp_decay: 0.0,
            aging_start_age: 60,
        }
    }
}

// ============================================================================
// 老化效果值
// ============================================================================

/// 当前老化效果值（计算后的具体数值）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgingEffectValues {
    /// 体力上限衰减值
    pub stamina_penalty: f32,

    /// HP 上限衰减值
    pub hp_penalty: f32,

    /// 衰老阶段描述
    pub stage: AgingStage,
}

/// 衰老阶段
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgingStage {
    /// 壮年（无影响）
    Prime,
    /// 中年（轻微影响）
    MiddleAge,
    /// 老年（明显影响）
    Old,
    /// 耄耋（严重影响）
    Venerable,
}

impl Default for AgingStage {
    fn default() -> Self {
        Self::Prime
    }
}

impl std::fmt::Display for AgingStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Prime => write!(f, "壮年"),
            Self::MiddleAge => write!(f, "中年"),
            Self::Old => write!(f, "老年"),
            Self::Venerable => write!(f, "耄耋"),
        }
    }
}

// ============================================================================
// 寿命状态
// ============================================================================

/// 寿命状态
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LifespanStatus {
    /// 存活
    Alive {
        /// 当前年龄
        age: u8,
    },
    /// 老化中（有属性惩罚）
    Aging {
        /// 当前年龄
        age: u8,
        /// 老化效果
        effects: AgingEffectValues,
    },
    /// 寿终（Agent 死亡，客户端停止运行）
    Deceased {
        /// 寿终年龄
        age: u8,
    },
}

impl LifespanStatus {
    /// 创建存活状态
    pub fn alive(age: u8) -> Self {
        Self::Alive { age }
    }

    /// 创建老化状态
    pub fn aging(age: u8, effects: AgingEffectValues) -> Self {
        Self::Aging { age, effects }
    }

    /// 创建寿终状态
    pub fn deceased(age: u8) -> Self {
        Self::Deceased { age }
    }

    /// 获取当前年龄
    pub fn age(&self) -> u8 {
        match self {
            Self::Alive { age } => *age,
            Self::Aging { age, .. } => *age,
            Self::Deceased { age } => *age,
        }
    }

    /// 是否存活
    pub fn is_alive(&self) -> bool {
        !matches!(self, Self::Deceased { .. })
    }

    /// 是否老化中
    pub fn is_aging(&self) -> bool {
        matches!(self, Self::Aging { .. })
    }

    /// 是否寿终
    pub fn is_deceased(&self) -> bool {
        matches!(self, Self::Deceased { .. })
    }
}

// ============================================================================
// 寿命配置
// ============================================================================

fn default_max_age() -> u8 {
    80
}
fn default_aging_rate() -> f32 {
    1.0
}

/// 寿命配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifespanConfig {
    /// 最大寿命（岁）
    #[serde(default = "default_max_age")]
    pub max_age: u8,

    /// 衰老速率：每经过多少游戏年，角色增加 1 岁
    /// 默认 1.0 = 现实 72 小时 = 1 游戏年 = 1 岁
    #[serde(default = "default_aging_rate")]
    pub aging_rate: f32,

    /// 老化对属性的影响（仅用于叙事描述）
    #[serde(default)]
    pub aging_effects: AgingEffects,

    /// 初始年龄（注册时的年龄）
    #[serde(default = "default_initial_age")]
    pub initial_age: u8,
}

fn default_initial_age() -> u8 {
    28
}

impl Default for LifespanConfig {
    fn default() -> Self {
        Self {
            max_age: 80,
            aging_rate: 1.0,
            aging_effects: AgingEffects::default(),
            initial_age: 28,
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
    fn test_lifespan_status_alive() {
        let status = LifespanStatus::alive(25);
        assert!(status.is_alive());
        assert!(!status.is_aging());
        assert!(!status.is_deceased());
        assert_eq!(status.age(), 25);
    }

    #[test]
    fn test_lifespan_status_aging() {
        let effects = AgingEffectValues {
            stamina_penalty: 10.0,
            hp_penalty: 5.0,
            stage: AgingStage::Old,
        };
        let status = LifespanStatus::aging(65, effects);
        assert!(status.is_alive());
        assert!(status.is_aging());
        assert!(!status.is_deceased());
        assert_eq!(status.age(), 65);
    }

    #[test]
    fn test_lifespan_status_deceased() {
        let status = LifespanStatus::deceased(85);
        assert!(!status.is_alive());
        assert!(!status.is_aging());
        assert!(status.is_deceased());
        assert_eq!(status.age(), 85);
    }

    #[test]
    fn test_lifespan_config_default() {
        let config = LifespanConfig::default();
        assert_eq!(config.max_age, 80);
        assert_eq!(config.aging_rate, 1.0);
        assert_eq!(config.initial_age, 28);
        assert_eq!(config.aging_effects.aging_start_age, 60);
    }

    #[test]
    fn test_aging_stage_display() {
        assert_eq!(format!("{}", AgingStage::Prime), "壮年");
        assert_eq!(format!("{}", AgingStage::MiddleAge), "中年");
        assert_eq!(format!("{}", AgingStage::Old), "老年");
        assert_eq!(format!("{}", AgingStage::Venerable), "耄耋");
    }
}
