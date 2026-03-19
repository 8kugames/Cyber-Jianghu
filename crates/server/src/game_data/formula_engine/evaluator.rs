// ============================================================================
// 公式计算器 - 递归下降解析器
// ============================================================================

use super::types::{Function, Operator, Token};
use anyhow::{Context, Result};

/// 公式计算器
pub struct Evaluator;

impl Evaluator {
    /// 语法分析并计算（入口）
    pub fn evaluate(tokens: &[Token]) -> Result<f64> {
        let mut pos = 0;
        Self::parse_expression(tokens, &mut pos)
    }

    /// 解析表达式（处理 + 和 -）
    fn parse_expression(tokens: &[Token], pos: &mut usize) -> Result<f64> {
        let mut left = Self::parse_term(tokens, pos)?;

        while *pos < tokens.len() {
            match &tokens[*pos] {
                Token::Operator(Operator::Add) => {
                    *pos += 1;
                    let right = Self::parse_term(tokens, pos)?;
                    left += right;
                }
                Token::Operator(Operator::Sub) => {
                    *pos += 1;
                    let right = Self::parse_term(tokens, pos)?;
                    left -= right;
                }
                _ => break,
            }
        }

        Ok(left)
    }

    /// 解析项（处理 * 和 /）
    fn parse_term(tokens: &[Token], pos: &mut usize) -> Result<f64> {
        let mut left = Self::parse_factor(tokens, pos)?;

        while *pos < tokens.len() {
            match &tokens[*pos] {
                Token::Operator(Operator::Mul) => {
                    *pos += 1;
                    let right = Self::parse_factor(tokens, pos)?;
                    left *= right;
                }
                Token::Operator(Operator::Div) => {
                    *pos += 1;
                    let right = Self::parse_factor(tokens, pos)?;
                    if right == 0.0 {
                        anyhow::bail!("除零错误");
                    }
                    left /= right;
                }
                _ => break,
            }
        }

        Ok(left)
    }

    /// 解析因子（处理数字、括号、函数）
    fn parse_factor(tokens: &[Token], pos: &mut usize) -> Result<f64> {
        if *pos >= tokens.len() {
            anyhow::bail!("表达式意外结束");
        }

        match &tokens[*pos] {
            Token::Number(n) => {
                let value = *n;
                *pos += 1;
                Ok(value)
            }
            Token::LeftParen => {
                *pos += 1; // 跳过 '('
                let value = Self::parse_expression(tokens, pos)?;
                if *pos >= tokens.len() || !matches!(tokens[*pos], Token::RightParen) {
                    anyhow::bail!("缺少右括号");
                }
                *pos += 1; // 跳过 ')'
                Ok(value)
            }
            Token::Function(func) => {
                let func = *func;
                *pos += 1; // 跳过函数名

                if *pos >= tokens.len() || !matches!(tokens[*pos], Token::LeftParen) {
                    anyhow::bail!("函数后缺少左括号");
                }
                *pos += 1; // 跳过 '('

                // 解析第一个参数
                let arg1 = Self::parse_expression(tokens, pos)?;

                // 检查是否有第二个参数
                let arg2 = if *pos < tokens.len() && matches!(tokens[*pos], Token::Comma) {
                    *pos += 1; // 跳过 ','
                    Some(Self::parse_expression(tokens, pos)?)
                } else {
                    None
                };

                if *pos >= tokens.len() || !matches!(tokens[*pos], Token::RightParen) {
                    anyhow::bail!("函数后缺少右括号");
                }
                *pos += 1; // 跳过 ')'

                // 执行函数
                match func {
                    Function::Max => {
                        let arg2 = arg2.context("max 函数需要两个参数")?;
                        Ok(arg1.max(arg2))
                    }
                    Function::Min => {
                        let arg2 = arg2.context("min 函数需要两个参数")?;
                        Ok(arg1.min(arg2))
                    }
                    Function::Floor => Ok(arg1.floor()),
                    Function::Ceil => Ok(arg1.ceil()),
                }
            }
            Token::Operator(Operator::Sub) => {
                *pos += 1; // 跳过 '-'
                let value = Self::parse_factor(tokens, pos)?;
                Ok(-value)
            }
            _ => anyhow::bail!("意外的 token: {:?}", tokens[*pos]),
        }
    }
}
