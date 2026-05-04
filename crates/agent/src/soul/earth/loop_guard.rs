// ============================================================================
// LoopGuard — Tool Call 循环检测
// ============================================================================
//
// 追踪每次 tool call，检测连续重复调用。
// Warn → Terminate 升级机制：先警告，再终止。
// pending_warning 在每次 check() 开头清除，防止跨 tool_calls 泄漏。

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
        // 每次调用先清除上一次的 pending warning，防止跨 tool_calls 泄漏
        self.pending_warning.take();

        self.history.push(tool_name.to_string());

        if self.history.len() > self.max_total {
            return LoopGuardAction::Terminate;
        }

        let consecutive = self.count_consecutive(tool_name);
        if consecutive >= self.max_same_tool {
            if !self.warned_tools.contains(tool_name) {
                self.warned_tools.insert(tool_name.to_string());
                let msg = format!(
                    "你已经连续{}次调用'{}'工具。如果之前的调用没有获得满意结果，\
                    请尝试其他方式或直接基于已有信息做出决策。",
                    consecutive, tool_name
                );
                self.pending_warning = Some(msg.clone());
                return LoopGuardAction::Warn(msg);
            }
            return LoopGuardAction::Terminate;
        }

        LoopGuardAction::Proceed
    }

    /// 消费式取出待注入的警告文本（取后清空）
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
