// ============================================================================
// Token 类型定义
// ============================================================================

/// Token 类型
#[derive(Debug, Clone)]
pub enum Token {
    Number(f64),
    Operator(Operator),
    Function(Function),
    LeftParen,
    RightParen,
    Comma,
}

/// 运算符
#[derive(Debug, Clone, Copy)]
pub enum Operator {
    Add,
    Sub,
    Mul,
    Div,
}

/// 函数
#[derive(Debug, Clone, Copy)]
pub enum Function {
    Max,
    Min,
    Floor,
    Ceil,
}
