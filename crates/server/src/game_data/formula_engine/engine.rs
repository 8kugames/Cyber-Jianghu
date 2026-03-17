// ============================================================================
// 公式引擎 - 主入口和变量替换
// ============================================================================

use super::context::PrimaryAttributeProvider;
use super::evaluator::Evaluator;
use super::parser::Parser;
use anyhow::Result;

/// 公式引擎（数据驱动，支持任意属性名）
#[derive(Debug, Clone)]
pub struct FormulaEngine;

impl Default for FormulaEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl FormulaEngine {
    /// 创建新的公式引擎（数据驱动，无需硬编码属性列表）
    pub fn new() -> Self {
        Self
    }

    /// 计算公式
    ///
    /// # 参数
    /// - `formula`: 公式字符串，如 "100 + constitution * 2"
    /// - `provider`: 主属性值提供者
    ///
    /// # 返回
    /// 计算结果（f64）
    pub fn evaluate(&self, formula: &str, provider: &dyn PrimaryAttributeProvider) -> Result<f64> {
        // 1. 变量替换
        let expanded = self.replace_variables(formula, provider)?;

        // 2. 解析并计算
        let tokens = Parser::tokenize(&expanded)?;
        let result = Evaluator::evaluate(&tokens)?;

        Ok(result)
    }

    /// 变量替换（数据驱动：动态提取变量名，而非硬编码）
    fn replace_variables(
        &self,
        formula: &str,
        provider: &dyn PrimaryAttributeProvider,
    ) -> Result<String> {
        let mut result = formula.to_string();
        let mut replaced = std::collections::HashSet::new();

        // 动态提取公式中的所有变量名（字母开头的标识符）
        let mut chars = formula.chars().peekable();
        while let Some(&ch) = chars.peek() {
            if ch.is_alphabetic() {
                let mut var_name = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_alphabetic() || c == '_' {
                        var_name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }

                // 检查是否是函数名（已知的函数列表）
                if matches!(var_name.as_str(), "max" | "min" | "floor" | "ceil") {
                    continue;
                }

                // 从 provider 获取变量值
                if let Some(value) = provider.get_attribute(&var_name) {
                    // 只替换每个变量一次，避免重复替换
                    if !replaced.contains(&var_name) {
                        result = result.replace(&var_name, &value.to_string());
                        replaced.insert(var_name);
                    }
                }
            } else {
                chars.next();
            }
        }

        Ok(result)
    }

    /// 验证公式语法是否正确
    ///
    /// # 参数
    /// - `formula`: 公式字符串
    /// - `known_attributes`: 可选的已知属性名列表，用于验证公式中的变量引用
    ///
    /// # 返回
    /// - `Ok(())`: 公式语法正确
    /// - `Err`: 公式语法错误或包含未知的属性名
    pub fn validate_formula(&self, formula: &str, known_attributes: Option<&[&str]>) -> Result<()> {
        // 创建一个动态的属性提供者
        struct DynamicMockProvider<'a> {
            known_attributes: Option<&'a [&'a str]>,
        }

        impl<'a> PrimaryAttributeProvider for DynamicMockProvider<'a> {
            fn get_attribute(&self, name: &str) -> Option<u8> {
                // 如果提供了已知属性列表，只对这些属性返回有效值
                // 否则对所有看起来像属性名的标识符返回默认值
                if let Some(attrs) = self.known_attributes {
                    if attrs.contains(&name) {
                        Some(10)
                    } else {
                        None
                    }
                } else {
                    // 没有提供属性列表时，对所有字母开头的标识符返回默认值
                    // 这样可以验证公式语法，而不验证具体的属性名
                    Some(10)
                }
            }
        }

        let provider = DynamicMockProvider { known_attributes };

        // 尝试计算公式
        self.evaluate(formula, &provider)?;
        Ok(())
    }
}
