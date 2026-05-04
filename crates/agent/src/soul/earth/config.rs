// ============================================================================
// EarthSoul 配置 — Tool Result Budget & Loop Guard
// ============================================================================
//
// 所有阈值从 agent.yaml 读取，零魔法值。
// enabled: true（默认）确保新安装的 agent 自动获得防护。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EarthSoulConfig {
    #[serde(default)]
    pub tool_budget: ToolBudgetConfig,
    #[serde(default)]
    pub loop_guard: LoopGuardConfig,
}

impl EarthSoulConfig {
    /// 校验所有配置值的合法性（Fail Fast）
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.tool_budget.enabled {
            anyhow::ensure!(
                self.tool_budget.aggregate_max_chars >= 100,
                "earth_soul.tool_budget.aggregate_max_chars 必须 >= 100，当前: {}",
                self.tool_budget.aggregate_max_chars
            );
            anyhow::ensure!(
                self.tool_budget.default_max_result_chars >= 50,
                "earth_soul.tool_budget.default_max_result_chars 必须 >= 50，当前: {}",
                self.tool_budget.default_max_result_chars
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolBudgetConfig {
    pub enabled: bool,
    pub default_max_result_chars: usize,
    pub aggregate_max_chars: usize,
    pub per_tool: HashMap<String, usize>,
}

impl Default for ToolBudgetConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_max_result_chars: 2000,
            aggregate_max_chars: 8000,
            per_tool: HashMap::new(),
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
            max_same_tool_consecutive: 3,
            max_total_calls: 10,
        }
    }
}
