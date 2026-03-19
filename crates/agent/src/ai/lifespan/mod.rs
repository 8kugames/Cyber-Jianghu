//! 寿命系统
//!
//! 用于叙事的年龄追踪和生命周期管理
//!
//! 说明：此系统主要用于叙事和记忆，不影响服务端游戏逻辑。

pub mod calculator;
pub mod types;

pub use calculator::LifespanCalculator;
pub use types::{AgingEffectValues, AgingEffects, AgingStage, LifespanConfig, LifespanStatus};
