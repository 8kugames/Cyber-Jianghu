// ============================================================================
// EarthSoul 配置 — Tool Result Budget & Loop Guard
// ============================================================================
//
// Tool budget 从 context_window_tokens 推导（per_tool_ratio × aggregate_ratio），
// 不独立硬编码字符数阈值。数据驱动，零魔法值。
// enabled: true（默认）确保新安装的 agent 自动获得防护。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EarthSoulConfig {
    #[serde(default)]
    pub tool_budget: ToolBudgetConfig,
    #[serde(default)]
    pub loop_guard: LoopGuardConfig,
    /// LLM tool-calling 最大轮次（默认 5，确保有足够轮次查询动作详情后再决策）
    #[serde(default = "default_max_tool_rounds")]
    pub max_tool_rounds: usize,
    /// 地魂 tool calling 日志配置（默认关闭）
    #[serde(default)]
    pub tool_log: ToolLogConfig,
}

fn default_max_tool_rounds() -> usize {
    5
}

impl EarthSoulConfig {
    /// 校验所有配置值的合法性（Fail Fast）
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.tool_budget.enabled {
            anyhow::ensure!(
                (0.01..=0.50).contains(&self.tool_budget.per_tool_ratio),
                "earth_soul.tool_budget.per_tool_ratio 必须在 0.01..=0.50 范围内，当前: {}",
                self.tool_budget.per_tool_ratio
            );
            anyhow::ensure!(
                (0.01..=0.50).contains(&self.tool_budget.aggregate_ratio),
                "earth_soul.tool_budget.aggregate_ratio 必须在 0.01..=0.50 范围内，当前: {}",
                self.tool_budget.aggregate_ratio
            );
            anyhow::ensure!(
                self.tool_budget.per_tool_ratio <= self.tool_budget.aggregate_ratio,
                "earth_soul.tool_budget.per_tool_ratio ({}) 必须 <= aggregate_ratio ({})",
                self.tool_budget.per_tool_ratio,
                self.tool_budget.aggregate_ratio,
            );
        }
        if self.tool_log.enabled {
            anyhow::ensure!(
                self.tool_log.max_calls_per_cycle >= 1,
                "earth_soul.tool_log.max_calls_per_cycle 必须 >= 1，当前: {}",
                self.tool_log.max_calls_per_cycle
            );
        }
        if self.loop_guard.enabled {
            anyhow::ensure!(
                self.loop_guard.max_same_tool_consecutive >= 1,
                "earth_soul.loop_guard.max_same_tool_consecutive 必须 >= 1，当前: {}",
                self.loop_guard.max_same_tool_consecutive
            );
            anyhow::ensure!(
                self.loop_guard.max_total_calls >= 1,
                "earth_soul.loop_guard.max_total_calls 必须 >= 1，当前: {}",
                self.loop_guard.max_total_calls
            );
        }
        Ok(())
    }
}

/// Tool result budget 配置 — 从 context_window_tokens 推导
///
/// 推导公式: `chars = context_window_tokens × ratio × 4 (chars/token)`
/// 不再使用独立硬编码的 default_max_result_chars / aggregate_max_chars。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolBudgetConfig {
    pub enabled: bool,
    /// 单条 tool result 占 context window 的比例
    #[serde(default = "default_per_tool_ratio")]
    pub per_tool_ratio: f64,
    /// 单次 loop 所有 tool results 占 context window 的比例
    #[serde(default = "default_aggregate_ratio")]
    pub aggregate_ratio: f64,
}

fn default_per_tool_ratio() -> f64 {
    0.03
}
fn default_aggregate_ratio() -> f64 {
    0.10
}

impl Default for ToolBudgetConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            per_tool_ratio: default_per_tool_ratio(),
            aggregate_ratio: default_aggregate_ratio(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoopGuardConfig {
    pub enabled: bool,
    pub max_same_tool_consecutive: usize,
    pub max_total_calls: usize,
}

impl Default for LoopGuardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_same_tool_consecutive: 2,
            max_total_calls: 6,
        }
    }
}

/// 地魂 tool calling 日志配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolLogConfig {
    /// 是否收集 tool call 日志（默认关闭，显式开启）
    pub enabled: bool,
    /// 结果摘要截断长度（截断 raw_result，非 budget 处理后）
    pub max_result_summary_chars: usize,
    /// 单 cycle 最多记录条数（上限防护）
    pub max_calls_per_cycle: usize,
}

impl Default for ToolLogConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_result_summary_chars: 200,
            max_calls_per_cycle: 50,
        }
    }
}
