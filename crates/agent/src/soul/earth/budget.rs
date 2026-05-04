// ============================================================================
// ToolResultBudget — Tool Result 截断预算
// ============================================================================
//
// 所有计数统一使用 .chars().count()（Unicode 标量值），与配置单位"字符"一致。
// 中文字符 1 char = 1 计数单位，不受 UTF-8 编码长度影响。

use super::config::ToolBudgetConfig;

pub struct ToolResultBudget {
    default_max: usize,
    aggregate_max: usize,
    per_tool: std::collections::HashMap<String, usize>,
    /// 已使用的字符预算（按 .chars().count() 累加）
    used: usize,
}

impl ToolResultBudget {
    pub fn new(config: &ToolBudgetConfig) -> Self {
        Self {
            default_max: config.default_max_result_chars,
            aggregate_max: config.aggregate_max_chars,
            per_tool: config.per_tool.clone(),
            used: 0,
        }
    }

    /// 截断 tool result 到预算范围内。
    /// 返回截断后的字符串，所有计数使用 .chars().count()。
    pub fn truncate(&mut self, tool_name: &str, result: &str) -> String {
        let per_tool_limit = self
            .per_tool
            .get(tool_name)
            .copied()
            .unwrap_or(self.default_max);
        let remaining = self.aggregate_max.saturating_sub(self.used);
        let effective_limit = per_tool_limit.min(remaining);

        let char_count = result.chars().count();
        if char_count <= effective_limit {
            self.used += char_count;
            return result.to_string();
        }

        // 预留 50 字符给截断标记（覆盖极端数字场景）
        let truncated_chars = effective_limit.saturating_sub(50);
        let truncated: String = result.chars().take(truncated_chars).collect();
        let truncated_count = truncated.chars().count();
        let marker = format!("\n[截断: 原{}字, 显示{}字]", char_count, truncated_count);
        let output = format!("{}...{}", truncated, marker);
        self.used += output.chars().count();
        output
    }

    pub fn is_exhausted(&self) -> bool {
        self.used >= self.aggregate_max
    }

    pub fn used_chars(&self) -> usize {
        self.used
    }

    /// 预算耗尽时的错误文本（替代空字符串）
    pub fn exhausted_message() -> &'static str {
        "[上下文预算耗尽: 工具结果总字符数已达上限，请基于已有信息做出决策]"
    }
}
