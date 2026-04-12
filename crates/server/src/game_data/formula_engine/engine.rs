// ============================================================================
// 公式引擎 - 基于 evalexpr 的统一公式求值
// ============================================================================
//
// 所有公式求值（派生属性、状态上限、伤害计算、恢复公式）都通过本引擎。
// 统一使用 evalexpr 作为后端，消灭系统中混用的两套逻辑。
// ============================================================================

use anyhow::{Context, Result};
use evalexpr::ContextWithMutableVariables;
use std::collections::HashMap;

/// 公式引擎
///
/// 基于 evalexpr 的薄封装，提供类型安全的公式求值接口。
/// 无状态结构，可自由 clone 或作为函数参数传递。
#[derive(Debug, Clone, Default)]
pub struct FormulaEngine;

impl FormulaEngine {
    pub fn new() -> Self {
        Self
    }

    /// 求值公式（f64 上下文，返回 f64）
    ///
    /// 适用于需要 float 变量的场景（如 weapon_multiplier）。
    pub fn evaluate(&self, formula: &str, context: &HashMap<String, f64>) -> Result<f64> {
        let mut eval_ctx = evalexpr::HashMapContext::<evalexpr::DefaultNumericTypes>::new();
        for (k, v) in context {
            let _ = eval_ctx.set_value(k.clone(), evalexpr::Value::Float(*v));
        }
        match evalexpr::eval_with_context(formula, &eval_ctx) {
            Ok(evalexpr::Value::Float(v)) => Ok(v),
            Ok(evalexpr::Value::Int(v)) => Ok(v as f64),
            Ok(other) => anyhow::bail!("公式返回非数值类型: {:?}", other),
            Err(e) => anyhow::bail!("公式解析失败: {} - 公式: {}", e, formula),
        }
    }

    /// 求值公式（i64 上下文，返回 i32）
    ///
    /// 适用于纯整数变量场景（属性恢复、伤害计算等）。
    /// Float 结果会被 floor 为 i32。
    pub fn evaluate_int(&self, formula: &str, context: &HashMap<String, i64>) -> Result<i32> {
        let mut eval_ctx = evalexpr::HashMapContext::<evalexpr::DefaultNumericTypes>::new();
        for (k, v) in context {
            let _ = eval_ctx.set_value(k.clone(), evalexpr::Value::Int(*v));
        }
        match evalexpr::eval_with_context(formula, &eval_ctx) {
            Ok(evalexpr::Value::Int(v)) => Ok(v as i32),
            Ok(evalexpr::Value::Float(v)) => Ok(v.floor() as i32),
            Ok(other) => anyhow::bail!("公式返回非数值类型: {:?}", other),
            Err(e) => anyhow::bail!("公式解析失败: {} - 公式: {}", e, formula),
        }
    }

    /// 求值带额外 float 变量的公式（i64 基础上下文 + f64 额外变量，返回 i32）
    ///
    /// 适用于混合 int/float 变量的场景（如伤害公式中的 weapon_bonus + weapon_multiplier）。
    pub fn evaluate_int_with_extras(
        &self,
        formula: &str,
        int_context: &HashMap<String, i64>,
        float_extras: &HashMap<String, f64>,
    ) -> Result<i32> {
        let mut eval_ctx = evalexpr::HashMapContext::<evalexpr::DefaultNumericTypes>::new();
        for (k, v) in int_context {
            let _ = eval_ctx.set_value(k.clone(), evalexpr::Value::Int(*v));
        }
        for (k, v) in float_extras {
            let _ = eval_ctx.set_value(k.clone(), evalexpr::Value::Float(*v));
        }
        match evalexpr::eval_with_context(formula, &eval_ctx) {
            Ok(evalexpr::Value::Int(v)) => Ok(v as i32),
            Ok(evalexpr::Value::Float(v)) => Ok(v.floor() as i32),
            Ok(other) => anyhow::bail!("公式返回非数值类型: {:?}", other),
            Err(e) => anyhow::bail!("公式解析失败: {} - 公式: {}", e, formula),
        }
    }

    /// 求值状态属性上限公式（带 fallback）
    ///
    /// 公式求值失败时尝试直接 parse 为 f32，再 fallback 到 default_max。
    pub fn evaluate_max(
        &self,
        formula: &Option<String>,
        default_max: f32,
        context: &HashMap<String, i64>,
    ) -> f32 {
        if let Some(f) = formula {
            let mut eval_ctx = evalexpr::HashMapContext::<evalexpr::DefaultNumericTypes>::new();
            for (k, v) in context {
                let _ = eval_ctx.set_value(k.clone(), evalexpr::Value::Int(*v));
            }
            match evalexpr::eval_with_context(f, &eval_ctx) {
                Ok(evalexpr::Value::Int(v)) => return v as f32,
                Ok(evalexpr::Value::Float(v)) => return v as f32,
                _ => {
                    // 尝试直接解析为数字（如 "255" 这种纯数字公式）
                    if let Ok(parsed) = f.parse::<f32>() {
                        return parsed;
                    }
                }
            }
        }
        default_max
    }

    /// 验证公式语法是否正确
    pub fn validate_formula(&self, formula: &str, known_attributes: Option<&[&str]>) -> Result<()> {
        let mut eval_ctx = evalexpr::HashMapContext::<evalexpr::DefaultNumericTypes>::new();

        if let Some(attrs) = known_attributes {
            for attr in attrs {
                let _ = eval_ctx.set_value(attr.to_string(), evalexpr::Value::Int(10));
            }
        }

        evalexpr::eval_with_context(formula, &eval_ctx)
            .with_context(|| format!("公式验证失败: {}", formula))?;
        Ok(())
    }
}
