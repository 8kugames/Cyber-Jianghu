//! 规则引擎验证器模块
//!
//! 提供基于规则的快速验证，无需 LLM 调用。
//!
//! # 架构
//!
//! - `types`: 规则类型定义
//! - `registry`: 规则注册表
//! - `evaluator`: 规则条件评估器
//! - `engine`: 规则引擎核心
//!
//! # 使用示例
//!
//! ```rust,no_run
//! use anyhow::Result;
//! use cyber_jianghu_agent::soul::reflector::rule_engine::{RuleEngine, RuleValidationContext};
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let engine = RuleEngine::new();
//!     let context: RuleValidationContext = unimplemented!();
//!     let _result = engine.validate_context(&context).await?;
//!     Ok(())
//! }
//! ```

// 重新导出所有子模块
pub mod engine;
pub mod evaluator;
pub mod registry;
pub mod types;

// 重新导出常用类型
pub use engine::RuleEngine;
pub use evaluator::{ConditionEvaluator, DefaultEvaluator};
pub use registry::RuleRegistry;
pub use types::{
    Rule, RuleCondition, RuleEngineConfig, RuleType, RuleValidationContext, RuleValidationResult,
};
