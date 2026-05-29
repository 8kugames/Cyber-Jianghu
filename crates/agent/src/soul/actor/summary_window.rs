// ============================================================================
// 叙事摘要窗口 - 滑动上下文优化
// ============================================================================
//
// 保留最近 N 轮的认知结果摘要，用于在 prompt 中注入近期认知轨迹。
// 帮助 LLM 理解连续决策的上下文，避免"失忆"。
//
// 窗口大小可配置，默认 3 轮（可覆盖大多数短期决策模式）
// ============================================================================

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// 叙事摘要窗口
///
/// 环形缓冲区，保留最近 N 轮的认知结果摘要。
pub struct NarrativeSummaryWindow {
    /// 窗口大小（默认 5）
    max_size: usize,
    /// 摘要队列
    summaries: VecDeque<NarrativeSummary>,
}

impl Default for NarrativeSummaryWindow {
    fn default() -> Self {
        Self::new(5)
    }
}

impl NarrativeSummaryWindow {
    /// 创建新的叙事摘要窗口
    ///
    /// # Arguments
    /// * `max_size` - 窗口大小，建议 5-7
    pub fn new(max_size: usize) -> Self {
        Self {
            max_size: max_size.max(1),
            summaries: VecDeque::with_capacity(max_size),
        }
    }

    /// 添加新的摘要到窗口
    pub fn push(&mut self, summary: NarrativeSummary, _validated: bool) {
        // 超过窗口大小时移除最旧的
        if self.summaries.len() >= self.max_size {
            self.summaries.pop_front();
        }
        self.summaries.push_back(summary);
    }

    /// 获取窗口中的摘要数量
    pub fn len(&self) -> usize {
        self.summaries.len()
    }

    /// 检查窗口是否为空
    pub fn is_empty(&self) -> bool {
        self.summaries.is_empty()
    }

    /// 获取最近的摘要
    pub fn latest(&self) -> Option<&NarrativeSummary> {
        self.summaries.back()
    }

    /// 获取所有摘要的引用
    pub fn get_all(&self) -> Vec<&NarrativeSummary> {
        self.summaries.iter().collect()
    }

    /// 获取最近 N 条同 action_type 的 validated 摘要的 full_decision
    ///
    /// 用于语义去重：提取最近同类意图的完整内容，供 LLM 比较语义相似度。
    /// 匹配规则：full_decision 以 "action_type:" 或 "action_type：" 开头，
    /// 或 full_decision 等于 action_type（无 content 的纯动作）。
    pub fn get_recent_same_type_decisions(&self, action_type: &str, limit: usize) -> Vec<String> {
        self.summaries
            .iter()
            .rev()
            .filter(|s| s.validated)
            .filter(|s| {
                s.full_decision == action_type
                    || s.full_decision.starts_with(&format!("{}:", action_type))
                    || s.full_decision.starts_with(&format!("{}：", action_type))
            })
            .take(limit)
            .map(|s| s.full_decision.clone())
            .collect()
    }

    /// 清空窗口
    pub fn clear(&mut self) {
        self.summaries.clear();
    }

    /// 更新最近一条 validated=true 摘要的 outcome
    ///
    /// Intent 执行后由 lifecycle 调用，将 "执行中" 替换为实际结果。
    /// 只更新通过审查的记录，跳过 Rejected 记录（避免 outcome 错位）。
    pub fn update_last_outcome(&mut self, outcome: String) {
        for summary in self.summaries.iter_mut().rev() {
            if summary.validated && summary.outcome == "执行中" {
                summary.outcome = outcome;
                return;
            }
        }
    }

    /// 生成窗口摘要（用于 prompt 注入）
    ///
    /// 格式化为简洁的近期认知轨迹，帮助 LLM 理解连续决策上下文。
    pub fn to_context(&self) -> String {
        if self.summaries.is_empty() {
            return String::new();
        }

        let lines: Vec<String> = self
            .summaries
            .iter()
            .rev() // 从新到旧
            .enumerate()
            .map(|(i, s)| {
                let age = if i == 0 { "刚" } else { "之前" };
                if s.motivation.is_empty() {
                    format!(
                        "- [{}] {} → {} [{}]",
                        age, s.perception, s.decision, s.outcome
                    )
                } else {
                    format!(
                        "- [{}] {} | {} → {} [{}]",
                        age, s.perception, s.motivation, s.decision, s.outcome
                    )
                }
            })
            .collect();

        let result = format!(
            "\n### 近期认知轨迹（主观回忆，非客观事实）\n{}\n",
            lines.join("\n")
        );

        result
    }

    /// 检测行为重复并返回量化警告
    ///
    /// 当最近 N 个动作中单一动作占比超过阈值时，返回具体数据。
    /// 数据驱动，避免 LLM 忽略模糊的"避免重复"指令。
    /// 生成详细摘要（用于调试）
    pub fn to_detailed_context(&self) -> String {
        if self.summaries.is_empty() {
            return String::new();
        }

        let lines: Vec<String> = self
            .summaries
            .iter()
            .rev()
            .enumerate()
            .map(|(i, s)| {
                let age = if i == 0 { "最近" } else { "之前" };
                format!(
                    "【{}】\n  感知: {}\n  动机: {}\n  决策: {}\n  结果: {}",
                    age, s.perception, s.motivation, s.decision, s.outcome
                )
            })
            .collect();

        format!(
            "\n### 近期认知轨迹（主观回忆，非客观事实）\n{}\n",
            lines.join("\n")
        )
    }
}

/// 单轮叙事摘要
///
/// 包含该轮认知流程的关键输出，用于滑动窗口。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NarrativeSummary {
    /// Tick ID
    pub tick_id: i64,
    /// 感知摘要
    pub perception: String,
    /// 动机摘要
    pub motivation: String,
    /// 决策内容（完整，用于 prompt 显示和语义去重）
    pub decision: String,
    /// 完整决策内容（与 decision 相同，保留字段兼容性）
    pub full_decision: String,
    /// 执行结果
    pub outcome: String,
    /// 是否通过 ReflectorSoul 审查
    pub validated: bool,
}

impl NarrativeSummary {
    /// 创建简化的摘要
    ///
    /// `decision` 参数应为 "action_type: content" 格式，或纯 action_type。
    /// 两个字段（decision / full_decision）使用相同值，由调用方保证格式。
    pub fn simple(tick_id: i64, decision: &str, outcome: &str) -> Self {
        Self {
            tick_id,
            perception: String::new(),
            motivation: String::new(),
            decision: decision.to_string(),
            full_decision: decision.to_string(),
            outcome: outcome.to_string(),
            validated: true,
        }
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_window_push_and_evict() {
        let mut window = NarrativeSummaryWindow::new(3);

        window.push(NarrativeSummary::simple(1, "吃馒头", "成功"), true);
        assert_eq!(window.len(), 1);

        window.push(NarrativeSummary::simple(2, "喝水", "成功"), true);
        assert_eq!(window.len(), 2);

        window.push(NarrativeSummary::simple(3, "休息", "成功"), true);
        assert_eq!(window.len(), 3);

        // 超出窗口大小，应移除最旧的
        window.push(NarrativeSummary::simple(4, "移动", "成功"), true);
        assert_eq!(window.len(), 3);

        // 验证最新的是 tick 4，最旧的是 tick 2
        assert_eq!(window.latest().unwrap().tick_id, 4);
        assert_eq!(window.get_all().first().unwrap().tick_id, 2);
    }

    #[test]
    fn test_to_context() {
        let mut window = NarrativeSummaryWindow::new(3);

        window.push(NarrativeSummary::simple(1, "吃馒头充饥", "成功"), true);
        window.push(NarrativeSummary::simple(2, "找水源", "失败"), true);

        let context = window.to_context();
        assert!(context.contains("近期认知轨迹"));
        assert!(context.contains("吃馒头"));
        assert!(context.contains("找水源"));
    }

    #[test]
    fn test_empty_window() {
        let window = NarrativeSummaryWindow::new(3);
        assert!(window.is_empty());
        assert_eq!(window.to_context(), "");
    }

    #[test]
    fn test_custom_size() {
        let mut window = NarrativeSummaryWindow::new(1);
        window.push(NarrativeSummary::simple(1, "A", "OK"), true);
        window.push(NarrativeSummary::simple(2, "B", "OK"), true);
        window.push(NarrativeSummary::simple(3, "C", "OK"), true);

        // 只保留 1 个
        assert_eq!(window.len(), 1);
        assert_eq!(window.latest().unwrap().tick_id, 3);
    }

}
