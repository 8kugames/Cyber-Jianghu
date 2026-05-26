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
use std::collections::HashMap;
use std::collections::VecDeque;

/// 行为重复检测：单一动作占比超过此比例触发警告
const REPETITION_RATIO_THRESHOLD: f64 = 0.5;
/// 行为重复检测：少于此样本数不检测
const REPETITION_MIN_SAMPLES: usize = 5;
/// 行为历史容量（独立于摘要窗口，仅追踪 action_type）
const ACTION_HISTORY_CAPACITY: usize = 20;

/// 行为历史记录（携带审查通过标记）
struct ActionRecord {
    action_type: String,
    /// 是否通过了 ReflectorSoul 审查
    validated: bool,
}

/// 叙事摘要窗口
///
/// 环形缓冲区，保留最近 N 轮的认知结果摘要。
pub struct NarrativeSummaryWindow {
    /// 窗口大小（默认 5）
    max_size: usize,
    /// 摘要队列
    summaries: VecDeque<NarrativeSummary>,
    /// 最近 action_type 记录（仅统计 validated=true 的，用于行为多样性检测）
    action_history: VecDeque<ActionRecord>,
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
            action_history: VecDeque::with_capacity(ACTION_HISTORY_CAPACITY),
        }
    }

    /// 添加新的摘要到窗口
    ///
    /// `validated` = true 表示已通过 ReflectorSoul 审查（action_type 合法）。
    /// 只有 validated=true 的记录参与行为重复检测。
    pub fn push(&mut self, summary: NarrativeSummary, validated: bool) {
        // 追踪 action_type 到独立历史（用于行为多样性检测）
        let action_type = summary.decision.clone();
        if self.action_history.len() >= ACTION_HISTORY_CAPACITY {
            self.action_history.pop_front();
        }
        self.action_history.push_back(ActionRecord {
            action_type,
            validated,
        });

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
        self.action_history.clear();
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
    /// 当检测到行为重复时，追加量化警告。
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
                    format!("- [{}] {} → {} [{}]", age, s.perception, s.decision, s.outcome)
                } else {
                    format!(
                        "- [{}] {} | {} → {} [{}]",
                        age, s.perception, s.motivation, s.decision, s.outcome
                    )
                }
            })
            .collect();

        let mut result = format!(
            "\n### 近期认知轨迹（主观回忆，非客观事实）\n{}\n",
            lines.join("\n")
        );

        // 行为重复警告（量化注入）
        if let Some(warning) = self.get_repetition_warning() {
            result.push_str(&format!("\n{}\n", warning));
        }

        result
    }

    /// 检测行为重复并返回量化警告
    ///
    /// 当最近 N 个动作中单一动作占比超过阈值时，返回具体数据。
    /// 数据驱动，避免 LLM 忽略模糊的"避免重复"指令。
    pub fn get_repetition_warning(&self) -> Option<String> {
        // 只统计 validated=true 的记录（通过 ReflectorSoul 审查的合法动作）
        let validated: Vec<&str> = self
            .action_history
            .iter()
            .filter(|r| r.validated)
            .map(|r| r.action_type.as_str())
            .collect();

        if validated.len() < REPETITION_MIN_SAMPLES {
            return None;
        }

        let mut counts: HashMap<&str, usize> = HashMap::new();
        for action in &validated {
            *counts.entry(action).or_insert(0) += 1;
        }

        let total = validated.len();
        let (&dominant_action, &dominant_count) = counts.iter().max_by_key(|&(_, &c)| c)?;

        let ratio = dominant_count as f64 / total as f64;
        if ratio >= REPETITION_RATIO_THRESHOLD && dominant_count >= REPETITION_MIN_SAMPLES {
            // 列出其他可用动作（仅 validated 的）
            let others: Vec<&str> = counts
                .keys()
                .filter(|&&a| a != dominant_action)
                .copied()
                .collect();
            let others_hint = if others.is_empty() {
                String::new()
            } else {
                format!("（如 {}）", others.join("、"))
            };

            return Some(format!(
                "[行为锁定警告] 你最近{}次行动中「{}」占{:.0}%（{}/{}），\
                 本轮必须执行不同行动{}。",
                total,
                dominant_action,
                ratio * 100.0,
                dominant_count,
                total,
                others_hint,
            ));
        }

        None
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
    /// 决策摘要（叙事意图）
    pub decision: String,
    /// 执行结果
    pub outcome: String,
    /// 是否通过 ReflectorSoul 审查
    pub validated: bool,
}

impl NarrativeSummary {
    /// 创建简化的摘要
    pub fn simple(tick_id: i64, decision: &str, outcome: &str) -> Self {
        Self {
            tick_id,
            perception: String::new(),
            motivation: String::new(),
            decision: decision.to_string(),
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

    // ========================================================================
    // 行为重复检测测试
    // ========================================================================

    #[test]
    fn test_no_warning_below_min_samples() {
        let mut window = NarrativeSummaryWindow::new(3);
        // 推入 4 个相同动作（低于 MIN_SAMPLES=5）
        for i in 1..=4 {
            window.push(NarrativeSummary::simple(i, "打坐", "成功"), true);
        }
        assert!(
            window.get_repetition_warning().is_none(),
            "少于 5 个样本不应触发警告"
        );
    }

    #[test]
    fn test_warning_on_repetition() {
        let mut window = NarrativeSummaryWindow::new(3);
        // 推入 8 个打坐 + 2 个其他（80% 打坐 > 50% 阈值）
        for i in 1..=8 {
            window.push(NarrativeSummary::simple(i, "打坐", "成功"), true);
        }
        window.push(NarrativeSummary::simple(9, "进食", "成功"), true);
        window.push(NarrativeSummary::simple(10, "移动", "成功"), true);

        let warning = window
            .get_repetition_warning()
            .expect("10 个样本中 80% 打坐应触发警告");
        assert!(
            warning.contains("打坐"),
            "警告应包含重复动作名称: {}",
            warning
        );
        assert!(
            warning.contains("行为锁定警告"),
            "警告应包含标签: {}",
            warning
        );
        assert!(
            warning.contains("进食") || warning.contains("移动"),
            "警告应提示可选动作: {}",
            warning
        );
    }

    #[test]
    fn test_no_warning_on_diverse_actions() {
        let mut window = NarrativeSummaryWindow::new(3);
        // 推入多种动作，无单一动作超过 50%
        window.push(NarrativeSummary::simple(1, "进食", "成功"), true);
        window.push(NarrativeSummary::simple(2, "移动", "成功"), true);
        window.push(NarrativeSummary::simple(3, "说话", "成功"), true);
        window.push(NarrativeSummary::simple(4, "采集", "成功"), true);
        window.push(NarrativeSummary::simple(5, "打坐", "成功"), true);
        window.push(NarrativeSummary::simple(6, "进食", "成功"), true);

        assert!(
            window.get_repetition_warning().is_none(),
            "多样化行为不应触发警告"
        );
    }

    #[test]
    fn test_warning_injected_into_context() {
        let mut window = NarrativeSummaryWindow::new(3);
        // 推入足够多重复动作触发警告
        for i in 1..=6 {
            window.push(NarrativeSummary::simple(i, "打坐", "成功"), true);
        }

        let context = window.to_context();
        assert!(
            context.contains("行为锁定警告"),
            "to_context() 应包含行为锁定警告: {}",
            context
        );
    }

    #[test]
    fn test_action_history_independent_of_window() {
        let mut window = NarrativeSummaryWindow::new(2);
        // 窗口大小 2，但 action_history 容量 20
        for i in 1..=8 {
            window.push(NarrativeSummary::simple(i, "打坐", "成功"), true);
        }

        // 窗口只保留 2 个摘要
        assert_eq!(window.len(), 2);
        // 但 action_history 保留 8 条，应触发警告
        assert!(
            window.get_repetition_warning().is_some(),
            "action_history 应独立于窗口大小追踪重复行为"
        );
    }

    #[test]
    fn test_clear_resets_action_history() {
        let mut window = NarrativeSummaryWindow::new(3);
        for i in 1..=6 {
            window.push(NarrativeSummary::simple(i, "打坐", "成功"), true);
        }
        assert!(window.get_repetition_warning().is_some());

        window.clear();
        assert!(
            window.get_repetition_warning().is_none(),
            "clear() 应重置 action_history"
        );
    }

    #[test]
    fn test_invalid_action_excluded_from_repetition() {
        let mut window = NarrativeSummaryWindow::new(3);
        // 5 个 validated=true 的打坐
        for i in 1..=5 {
            window.push(NarrativeSummary::simple(i, "打坐", "成功"), true);
        }
        // 5 个 validated=false 的"观察"（非法 action_type，被 ReflectorSoul 驳回）
        for i in 6..=10 {
            window.push(NarrativeSummary::simple(i, "观察", "失败"), false);
        }

        // "观察"不应出现在警告中，只统计 validated=true 的
        let warning = window.get_repetition_warning();
        assert!(warning.is_some(), "5 个 validated 打坐应触发警告");
        let warning = warning.unwrap();
        assert!(warning.contains("打坐"), "警告应包含打坐: {}", warning);
        assert!(
            !warning.contains("观察"),
            "警告不应包含未通过审查的动作: {}",
            warning
        );
    }
}
