// ============================================================================
// LoopGuard — Tool Call 循环检测
// ============================================================================
//
// 渐进策略：第 1 次重复调用 → 注入警告（Warn），第 2 次重复 → 截断（Terminate）。
// 警告文本通过 take_pending_warning() 取出，注入到 tool result 前缀。

use std::collections::HashSet;

use super::config::LoopGuardConfig;

pub enum LoopGuardAction {
    Proceed,
    Warn(String),
    Terminate,
}

pub struct LoopGuard {
    max_same_tool: usize,
    max_total: usize,
    history: Vec<String>,
    warned_tools: HashSet<String>,
    pending_warning: Option<String>,
}

impl LoopGuard {
    pub fn new(config: &LoopGuardConfig) -> Self {
        Self {
            max_same_tool: config.max_same_tool_consecutive,
            max_total: config.max_total_calls,
            history: Vec::new(),
            warned_tools: HashSet::new(),
            pending_warning: None,
        }
    }

    pub fn check(&mut self, tool_name: &str) -> LoopGuardAction {
        // 清除上一次的 pending warning，防止跨 tool_calls 泄漏
        self.pending_warning.take();

        self.history.push(tool_name.to_string());

        if self.history.len() > self.max_total {
            return LoopGuardAction::Terminate;
        }

        let consecutive = self.count_consecutive(tool_name);
        if consecutive >= self.max_same_tool {
            if !self.warned_tools.contains(tool_name) {
                // 第 1 次重复：警告
                self.warned_tools.insert(tool_name.to_string());
                let msg = format!(
                    "你已经连续{}次调用'{}'工具，结果相同。请直接基于已有信息做出决策，不要再重复调用。",
                    consecutive, tool_name
                );
                self.pending_warning = Some(msg.clone());
                return LoopGuardAction::Warn(msg);
            }
            // 第 2 次重复：截断
            return LoopGuardAction::Terminate;
        }

        LoopGuardAction::Proceed
    }

    /// 消费式取出待注入的警告文本
    pub fn take_pending_warning(&mut self) -> Option<String> {
        self.pending_warning.take()
    }

    fn count_consecutive(&self, tool_name: &str) -> usize {
        self.history
            .iter()
            .rev()
            .take_while(|t| *t == tool_name)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(max_same: usize, max_total: usize) -> LoopGuardConfig {
        LoopGuardConfig {
            enabled: true,
            max_same_tool_consecutive: max_same,
            max_total_calls: max_total,
        }
    }

    #[test]
    fn test_proceed_on_first_call() {
        let mut guard = LoopGuard::new(&config(2, 6));
        match guard.check("query_world") {
            LoopGuardAction::Proceed => {}
            other => panic!("Expected Proceed, got {:?}", action_name(&other)),
        }
    }

    #[test]
    fn test_warn_on_first_duplicate() {
        let mut guard = LoopGuard::new(&config(2, 6));
        guard.check("query_world"); // Proceed (consecutive=1)
        let action = guard.check("query_world"); // Warn (consecutive=2)
        match action {
            LoopGuardAction::Warn(msg) => {
                assert!(msg.contains("query_world"));
                assert!(msg.contains("连续2次"));
            }
            other => panic!("Expected Warn, got {:?}", action_name(&other)),
        }
        // pending_warning 应可用
        let warning = guard.take_pending_warning();
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("query_world"));
    }

    #[test]
    fn test_terminate_on_second_duplicate() {
        let mut guard = LoopGuard::new(&config(2, 6));
        guard.check("query_world"); // Proceed
        guard.check("query_world"); // Warn
        let _ = guard.take_pending_warning(); // 消费 warning
        match guard.check("query_world") {
            LoopGuardAction::Terminate => {}
            other => panic!("Expected Terminate, got {:?}", action_name(&other)),
        }
    }

    #[test]
    fn test_different_tools_tracked_independently() {
        let mut guard = LoopGuard::new(&config(2, 6));
        guard.check("query_world"); // Proceed
        guard.check("get_action_detail"); // Proceed (different tool)
        // query_world consecutive reset to 1
        match guard.check("query_world") {
            LoopGuardAction::Proceed => {}
            other => panic!("Expected Proceed, got {:?}", action_name(&other)),
        }
    }

    #[test]
    fn test_terminate_on_total_exceeded() {
        let mut guard = LoopGuard::new(&config(5, 3));
        guard.check("a"); // total=1
        guard.check("b"); // total=2
        guard.check("c"); // total=3
        match guard.check("d") {
            // total=4 > max_total=3
            LoopGuardAction::Terminate => {}
            other => panic!("Expected Terminate, got {:?}", action_name(&other)),
        }
    }

    #[test]
    fn test_pending_warning_cleared_on_next_check() {
        let mut guard = LoopGuard::new(&config(2, 6));
        guard.check("query_world");
        guard.check("query_world"); // Warn → sets pending_warning
        // 不消费 warning，直接调用另一个 tool
        guard.check("get_action_detail"); // check 开头 take() 清除 warning
        assert!(guard.take_pending_warning().is_none());
    }

    #[test]
    fn test_warned_tools_accumulate_across_different_tools() {
        let mut guard = LoopGuard::new(&config(2, 10));
        // Tool A: 2 consecutive → Warn
        guard.check("a");
        guard.check("a");
        let _ = guard.take_pending_warning();
        // Switch to tool B
        guard.check("b");
        // Back to tool A: consecutive=1 (reset by B)
        match guard.check("a") {
            LoopGuardAction::Proceed => {}
            other => panic!("Expected Proceed, got {:?}", action_name(&other)),
        }
        // Tool A again: consecutive=2, but already warned → Terminate
        match guard.check("a") {
            LoopGuardAction::Terminate => {}
            other => panic!("Expected Terminate, got {:?}", action_name(&other)),
        }
    }

    fn action_name(action: &LoopGuardAction) -> &'static str {
        match action {
            LoopGuardAction::Proceed => "Proceed",
            LoopGuardAction::Warn(_) => "Warn",
            LoopGuardAction::Terminate => "Terminate",
        }
    }
}
