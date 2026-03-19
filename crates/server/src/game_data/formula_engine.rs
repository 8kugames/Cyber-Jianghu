// ============================================================================
// 公式计算引擎
// ============================================================================
//
// 用于解析和计算派生属性公式，支持：
// - 基本数学运算：+, -, *, /, ()
// - 变量引用：strength, agility, constitution, intelligence, charisma, luck
// - 函数支持：max, min, floor, ceil
//
// 示例公式：
// - "100 + constitution * 2"
// - "50 + strength * 2"
// - "0.05 + agility * 0.005"
// ============================================================================

// 模块声明
mod context;
mod engine;
mod evaluator;
mod parser;
mod types;

// 测试模块
#[cfg(test)]
mod tests;

// 重新导出公共API
pub use context::PrimaryAttributeProvider;
pub use engine::FormulaEngine;
