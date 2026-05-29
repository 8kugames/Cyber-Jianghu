// ============================================================================
// ToolResultBudget — 从 context_window_tokens 推导的工具结果预算
// ============================================================================
//
// 预算不再硬编码字符数阈值，从 context_window_tokens × ratio 推导。
// 比例由 ToolBudgetConfig (agent.yaml) 配置，运行时结合实际 context window 计算。
//
// chars_per_token = 4: 中文混合文本经验值，与 ConversationHistory 一致。

use super::config::ToolBudgetConfig;

/// 中文混合文本 chars/tokens 转换因子
const CHARS_PER_TOKEN: f64 = 4.0;

pub struct ToolResultBudget {
    per_tool_limit: usize,
    aggregate_limit: usize,
    used: usize,
}

impl ToolResultBudget {
    /// 从 config + context_window_tokens 推导预算
    pub fn new(config: &ToolBudgetConfig, context_window_tokens: u32) -> Self {
        let cwt = context_window_tokens as f64;
        let per_tool_limit = (cwt * config.per_tool_ratio * CHARS_PER_TOKEN) as usize;
        let aggregate_limit = (cwt * config.aggregate_ratio * CHARS_PER_TOKEN) as usize;
        tracing::info!(
            "[budget] 初始化: per_tool={}, aggregate={}, context_window={}",
            per_tool_limit,
            aggregate_limit,
            context_window_tokens
        );
        Self {
            per_tool_limit,
            aggregate_limit,
            used: 0,
        }
    }

    /// JSON 感知的结果处理：先紧凑化，再字符截断兜底。
    pub fn process(&mut self, tool_name: &str, value: &serde_json::Value) -> String {
        let json_str = value.to_string();
        let char_count = json_str.chars().count();

        // 1. 直接 fits
        if self.fits(char_count) {
            self.used += char_count;
            return json_str;
        }

        // 2. JSON 结构紧凑化 — 传入 per_tool_limit 让 compactor 自适应
        let compacted =
            super::compactor::compact_tool_result(tool_name, value, self.per_tool_limit);
        let compact_str = compacted.to_string();
        let compact_count = compact_str.chars().count();

        if self.fits(compact_count) {
            tracing::info!(
                "[budget] {} 紧凑化: {} → {} chars (per_tool_limit={})",
                tool_name,
                char_count,
                compact_count,
                self.per_tool_limit
            );
            self.used += compact_count;
            return compact_str;
        }

        // 3. 字符截断兜底
        tracing::warn!(
            "[budget] {} 紧凑化后仍超预算: {}/{} chars",
            tool_name,
            compact_count,
            self.per_tool_limit
        );
        self.truncate(&compact_str)
    }

    /// 检查给定字符数是否在预算内
    fn fits(&self, char_count: usize) -> bool {
        let remaining = self.aggregate_limit.saturating_sub(self.used);
        char_count <= self.per_tool_limit.min(remaining)
    }

    pub fn is_exhausted(&self) -> bool {
        self.used >= self.aggregate_limit
    }

    pub fn used_chars(&self) -> usize {
        self.used
    }

    /// 预算耗尽时的错误文本
    pub fn exhausted_message() -> &'static str {
        "[上下文预算耗尽: 工具结果总字符数已达上限，请基于已有信息做出决策]"
    }

    /// 字符截断（内部 fallback）
    fn truncate(&mut self, result: &str) -> String {
        let remaining = self.aggregate_limit.saturating_sub(self.used);
        let effective_limit = self.per_tool_limit.min(remaining);

        let char_count = result.chars().count();
        if char_count <= effective_limit {
            self.used += char_count;
            return result.to_string();
        }

        let truncated_chars = effective_limit.saturating_sub(50);
        let truncated: String = result.chars().take(truncated_chars).collect();
        let truncated_count = truncated.chars().count();
        let marker = format!("\n[截断: 原{}字, 显示{}字]", char_count, truncated_count);
        let output = format!("{}...{}", truncated, marker);
        self.used += output.chars().count();
        output
    }
}
