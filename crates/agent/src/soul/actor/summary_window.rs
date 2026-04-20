// ============================================================================
// 叙事摘要窗口 - 滑动上下文优化
// ============================================================================
//
// 保留最近 N 轮的认知结果摘要，用于在 prompt 中注入近期行动轨迹。
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
    /// 窗口大小（默认 3）
    max_size: usize,
    /// 摘要队列
    summaries: VecDeque<NarrativeSummary>,
}

impl Default for NarrativeSummaryWindow {
    fn default() -> Self {
        Self::new(3)
    }
}

impl NarrativeSummaryWindow {
    /// 创建新的叙事摘要窗口
    ///
    /// # Arguments
    /// * `max_size` - 窗口大小，建议 3-5
    pub fn new(max_size: usize) -> Self {
        Self {
            max_size: max_size.max(1), // 至少保留 1 轮
            summaries: VecDeque::with_capacity(max_size),
        }
    }

    /// 添加新的摘要到窗口
    pub fn push(&mut self, summary: NarrativeSummary) {
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

    /// 清空窗口
    pub fn clear(&mut self) {
        self.summaries.clear();
    }

    /// 更新最近一条摘要的 outcome
    ///
    /// Intent 执行后由 lifecycle 调用，将 "执行中" 替换为实际结果。
    pub fn update_last_outcome(&mut self, outcome: String) {
        if let Some(summary) = self.summaries.back_mut() {
            summary.outcome = outcome;
        }
    }

    /// 生成窗口摘要（用于 prompt 注入）
    ///
    /// 格式化为简洁的近期行动轨迹，帮助 LLM 理解连续决策上下文。
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
                format!(
                    "- [{}] {} → {} [{}]",
                    age,
                    s.perception.chars().take(15).collect::<String>(),
                    s.decision.chars().take(20).collect::<String>(),
                    s.outcome
                )
            })
            .collect();

        format!("\n### 近期行动轨迹\n{}\n", lines.join("\n"))
    }

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

        format!("\n### 近期行动轨迹\n{}\n", lines.join("\n"))
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
    /// 决策摘要（叙事意图）
    pub decision: String,
    /// 执行结果
    pub outcome: String,
}

impl NarrativeSummary {
    /// 从 CognitiveChain 创建摘要
    ///
    /// 压缩各阶段输出为简短摘要。
    #[allow(dead_code)]
    pub fn from_chain(chain: &crate::soul::actor::CognitiveChain) -> Self {
        Self {
            tick_id: chain.tick_id,
            perception: chain
                .get_stage(crate::soul::actor::CognitiveStage::Perception)
                .map(|s| s.content.chars().take(50).collect())
                .unwrap_or_default(),
            motivation: chain
                .get_stage(crate::soul::actor::CognitiveStage::Motivation)
                .map(|s| s.content.chars().take(50).collect())
                .unwrap_or_default(),
            decision: chain
                .get_stage(crate::soul::actor::CognitiveStage::Decision)
                .map(|s| s.content.chars().take(50).collect())
                .unwrap_or_default(),
            outcome: String::new(), // 结果需要外部填充
        }
    }

    /// 创建简化的摘要
    pub fn simple(tick_id: i64, decision: &str, outcome: &str) -> Self {
        Self {
            tick_id,
            perception: String::new(),
            motivation: String::new(),
            decision: decision.to_string(),
            outcome: outcome.to_string(),
        }
    }

    /// 截断文本到指定长度（保留供将来使用）
    #[allow(dead_code)]
    fn truncate(text: &str, max_len: usize) -> String {
        if text.len() <= max_len {
            return text.to_string();
        }
        let end = text
            .char_indices()
            .nth(max_len.saturating_sub(3))
            .map(|(idx, _)| idx)
            .unwrap_or(text.len());
        format!("{}...", &text[..end])
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

        window.push(NarrativeSummary::simple(1, "吃馒头", "成功"));
        assert_eq!(window.len(), 1);

        window.push(NarrativeSummary::simple(2, "喝水", "成功"));
        assert_eq!(window.len(), 2);

        window.push(NarrativeSummary::simple(3, "休息", "成功"));
        assert_eq!(window.len(), 3);

        // 超出窗口大小，应移除最旧的
        window.push(NarrativeSummary::simple(4, "移动", "成功"));
        assert_eq!(window.len(), 3);

        // 验证最新的是 tick 4，最旧的是 tick 2
        assert_eq!(window.latest().unwrap().tick_id, 4);
        assert_eq!(window.get_all().first().unwrap().tick_id, 2);
    }

    #[test]
    fn test_to_context() {
        let mut window = NarrativeSummaryWindow::new(3);

        window.push(NarrativeSummary::simple(1, "吃馒头充饥", "成功"));
        window.push(NarrativeSummary::simple(2, "找水源", "失败"));

        let context = window.to_context();
        assert!(context.contains("近期行动轨迹"));
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
        window.push(NarrativeSummary::simple(1, "A", "OK"));
        window.push(NarrativeSummary::simple(2, "B", "OK"));
        window.push(NarrativeSummary::simple(3, "C", "OK"));

        // 只保留 1 个
        assert_eq!(window.len(), 1);
        assert_eq!(window.latest().unwrap().tick_id, 3);
    }
}
