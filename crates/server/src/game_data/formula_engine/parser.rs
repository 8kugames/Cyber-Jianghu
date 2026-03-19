// ============================================================================
// 公式解析器 - 词法分析和语法分析
// ============================================================================

use super::types::{Function, Operator, Token};
use anyhow::{Context, Result};

/// 公式解析器
pub struct Parser;

impl Parser {
    /// 词法分析：将字符串转换为 Token 列表
    pub fn tokenize(formula: &str) -> Result<Vec<Token>> {
        let mut tokens = Vec::new();
        let mut chars = formula.chars().peekable();

        while let Some(&ch) = chars.peek() {
            if ch.is_whitespace() {
                chars.next();
                continue;
            }

            // 数字（整数或小数）
            if ch.is_ascii_digit() || ch == '.' {
                let mut num_str = String::new();
                let mut has_dot = false;

                while let Some(&c) = chars.peek() {
                    if c.is_ascii_digit() {
                        num_str.push(c);
                        chars.next();
                    } else if c == '.' && !has_dot {
                        num_str.push(c);
                        chars.next();
                        has_dot = true;
                    } else {
                        break;
                    }
                }

                let num: f64 = num_str
                    .parse()
                    .with_context(|| format!("无法解析数字: {}", num_str))?;
                tokens.push(Token::Number(num));
                continue;
            }

            // 函数名（字母开头）
            if ch.is_alphabetic() {
                let mut name = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_alphabetic() {
                        name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }

                // 检查是否是函数
                match name.as_str() {
                    "max" => tokens.push(Token::Function(Function::Max)),
                    "min" => tokens.push(Token::Function(Function::Min)),
                    "floor" => tokens.push(Token::Function(Function::Floor)),
                    "ceil" => tokens.push(Token::Function(Function::Ceil)),
                    _ => anyhow::bail!("未知的函数或变量: {}", name),
                }
                continue;
            }

            // 运算符和括号
            match ch {
                '+' => tokens.push(Token::Operator(Operator::Add)),
                '-' => tokens.push(Token::Operator(Operator::Sub)),
                '*' => tokens.push(Token::Operator(Operator::Mul)),
                '/' => tokens.push(Token::Operator(Operator::Div)),
                '(' => tokens.push(Token::LeftParen),
                ')' => tokens.push(Token::RightParen),
                ',' => tokens.push(Token::Comma),
                _ => anyhow::bail!("不支持的字符: {}", ch),
            }
            chars.next();
        }

        Ok(tokens)
    }
}
