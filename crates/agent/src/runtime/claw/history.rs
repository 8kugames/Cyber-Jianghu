// ============================================================================
// History Manager - 解决 Context Overflow 的关键组件
// ============================================================================
//
// 设计原则：
// 1. 严格限制历史消息数量，防止 context window 溢出
// 2. 支持 auto-compaction（LLM summarization）当阈值超过时
// 3. 使用 FIFO 驱逐策略作为兜底
// 4. 零信任：不允许静默失败
//
// 参照 ZeroClaw 的 History Management 设计
// ============================================================================

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::ai::llm::LlmClient;

// ============================================================================
// 常量 - 魔法数字必须可配置
// ============================================================================

/// 默认最大消息数
const DEFAULT_MAX_MESSAGES: usize = 50;

/// 默认 compaction 阈值（相对于 max_messages 的比例）
const DEFAULT_COMPACTION_THRESHOLD_RATIO: f64 = 0.8;

/// 最大 system prompt 长度
const DEFAULT_MAX_SYSTEM_PROMPT_LEN: usize = 2000;

/// 默认保留的最近消息数（compaction 后）
const DEFAULT_KEEP_RECENT_MESSAGES: usize = 10;

/// LLM summarization prompt template
const SUMMARIZATION_PROMPT_TEMPLATE: &str = r#"请将以下对话历史压缩为简洁的摘要，保留关键信息：

对话历史：
{history}

请生成一个不超过 200 字的摘要，包含：
1. 主要事件和动作
2. 关键决策
3. 重要的状态变化

摘要："#;

// ============================================================================
// 配置
// ============================================================================

/// History Manager 配置
#[derive(Debug, Clone)]
pub struct HistoryConfig {
    /// 最大消息数（不含 system prompt）
    pub max_messages: usize,
    /// Compaction 触发阈值（消息数）
    pub compaction_threshold: usize,
    /// System prompt 最大长度
    pub max_system_prompt_len: usize,
    /// Compaction 后保留的最近消息数
    pub keep_recent_messages: usize,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            max_messages: DEFAULT_MAX_MESSAGES,
            compaction_threshold: (DEFAULT_MAX_MESSAGES as f64 * DEFAULT_COMPACTION_THRESHOLD_RATIO)
                as usize,
            max_system_prompt_len: DEFAULT_MAX_SYSTEM_PROMPT_LEN,
            keep_recent_messages: DEFAULT_KEEP_RECENT_MESSAGES,
        }
    }
}

impl HistoryConfig {
    /// 从环境变量或配置文件加载（未来扩展）
    pub fn from_env() -> Self {
        Self::default() // 目前使用默认值，后续可扩展
    }
}

// ============================================================================
// 类型
// ============================================================================

/// 聊天消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// 角色：user, assistant, system, tool
    pub role: String,
    /// 内容
    pub content: String,
    /// 工具调用 ID（如果 role=tool）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// 工具名称（如果 role=tool）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

impl ChatMessage {
    /// 创建 user 消息
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
            tool_call_id: None,
            tool_name: None,
        }
    }

    /// 创建 assistant 消息
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            tool_call_id: None,
            tool_name: None,
        }
    }

    /// 创建 system 消息
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
            tool_call_id: None,
            tool_name: None,
        }
    }

    /// 创建 tool 消息
    pub fn tool(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            tool_name: Some(tool_name.into()),
        }
    }

    /// 检查是否是 system 消息
    pub fn is_system(&self) -> bool {
        self.role == "system"
    }

    /// 检查是否应该被计入消息数
    pub fn is_countable(&self) -> bool {
        self.role != "system"
    }
}

/// 对话历史条目（包含消息和元数据）
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub message: ChatMessage,
    pub timestamp: std::time::Instant,
}

impl HistoryEntry {
    pub fn new(message: ChatMessage) -> Self {
        Self {
            message,
            timestamp: std::time::Instant::now(),
        }
    }
}

// ============================================================================
// History Manager
// ============================================================================

/// History Manager - 管理对话历史，防止 context overflow
///
/// 设计原则：
/// 1. 严格限制消息数量
/// 2. 支持 auto-compaction
/// 3. FIFO 驱逐策略
/// 4. Fail Fast：不允许静默失败
pub struct HistoryManager {
    config: HistoryConfig,
    system_prompt: Option<ChatMessage>,
    messages: Vec<HistoryEntry>,
    summary: Option<String>,
}

impl Default for HistoryManager {
    fn default() -> Self {
        Self::new(HistoryConfig::default())
    }
}

impl Clone for HistoryManager {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            system_prompt: self.system_prompt.clone(),
            messages: self.messages.clone(),
            summary: self.summary.clone(),
        }
    }
}

impl HistoryManager {
    pub fn new(config: HistoryConfig) -> Self {
        Self {
            config,
            system_prompt: None,
            messages: Vec::new(),
            summary: None,
        }
    }

    /// 设置 system prompt
    ///
    /// # Fail Fast
    /// - 如果 prompt 超过配置的最大长度，返回错误
    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) -> Result<()> {
        let prompt_str = prompt.into();
        if prompt_str.len() > self.config.max_system_prompt_len {
            anyhow::bail!(
                "System prompt length {} exceeds max {}",
                prompt_str.len(),
                self.config.max_system_prompt_len
            );
        }
        self.system_prompt = Some(ChatMessage::system(prompt_str));
        Ok(())
    }

    /// 添加用户消息
    pub fn add_user_message(&mut self, content: impl Into<String>) {
        self.add_message(ChatMessage::user(content));
    }

    /// 添加助手消息
    pub fn add_assistant_message(&mut self, content: impl Into<String>) {
        self.add_message(ChatMessage::assistant(content));
    }

    /// 添加工具结果消息
    pub fn add_tool_message(
        &mut self,
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        content: impl Into<String>,
    ) {
        self.add_message(ChatMessage::tool(tool_call_id, tool_name, content));
    }

    /// 添加消息
    fn add_message(&mut self, message: ChatMessage) {
        self.messages.push(HistoryEntry::new(message));
        self.trim();
    }

    /// 获取当前消息数（不含 system prompt）
    pub fn message_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|e| e.message.is_countable())
            .count()
    }

    /// 检查是否需要 compaction
    pub fn needs_compaction(&self) -> bool {
        self.message_count() >= self.config.compaction_threshold
    }

    /// 执行 FIFO trim
    ///
    /// 移除最旧的消息直到在限制内
    fn trim(&mut self) {
        while self.message_count() > self.config.max_messages {
            // 移除最旧的非 system 消息
            if let Some(pos) = self.messages.iter().position(|e| e.message.is_countable()) {
                self.messages.remove(pos);
                debug!("History trim: removed message at position {}", pos);
            } else {
                // 理论上不应该发生，但防御性编程
                break;
            }
        }
    }

    /// 执行 auto-compaction（LLM summarization）
    ///
    /// # Fail Fast
    /// - 如果 provider 返回错误，panic（不允许静默失败）
    pub async fn compact(&mut self, provider: &dyn LlmClient) -> Result<()> {
        if !self.needs_compaction() {
            return Ok(());
        }

        debug!(
            "Starting history compaction: {} messages, threshold: {}",
            self.message_count(),
            self.config.compaction_threshold
        );

        // 构建历史摘要请求
        let history_text = self.build_history_for_compaction();
        let prompt = SUMMARIZATION_PROMPT_TEMPLATE.replace("{history}", &history_text);

        // 调用 LLM 生成摘要
        let summary = provider
            .complete(&prompt)
            .await
            .context("LLM compaction failed")?;

        debug!("Compaction summary generated: {} chars", summary.len());

        // 保留最近的 N 条消息 + 摘要
        let recent_count = self.config.keep_recent_messages;
        let messages_to_keep: Vec<_> = self
            .messages
            .iter()
            .rev()
            .take(recent_count)
            .cloned()
            .collect();

        self.messages = messages_to_keep.into_iter().rev().collect();
        self.summary = Some(summary);

        debug!(
            "Compaction complete: {} messages kept",
            self.message_count()
        );

        Ok(())
    }

    /// 构建用于 compaction 的历史文本
    fn build_history_for_compaction(&self) -> String {
        let mut lines = Vec::new();

        // 添加摘要（如果存在）
        if let Some(ref summary) = self.summary {
            lines.push(format!("【之前摘要】\n{}\n", summary));
        }

        // 添加所有消息
        for entry in &self.messages {
            lines.push(format!(
                "[{}] {}: {}",
                entry.timestamp.elapsed().as_secs(),
                entry.message.role,
                entry.message.content
            ));
        }

        lines.join("\n")
    }

    /// 构建完整的消息列表（用于 LLM 调用）
    pub fn build_messages(&self) -> Vec<ChatMessage> {
        let mut result = Vec::new();

        // 1. System prompt
        if let Some(ref system) = self.system_prompt {
            result.push(system.clone());
        }

        // 2. 摘要（如果存在）
        if let Some(ref summary) = self.summary {
            result.push(ChatMessage::system(format!("【对话摘要】\n{}", summary)));
        }

        // 3. 所有消息
        for entry in &self.messages {
            result.push(entry.message.clone());
        }

        result
    }

    /// 获取消息总数（用于调试）
    #[allow(dead_code)]
    pub fn total_messages(&self) -> usize {
        self.messages.len()
    }

    /// 检查健康状态（用于调试和监控）
    pub fn health_check(&self) -> HistoryHealth {
        let count = self.message_count();
        let ratio = count as f64 / self.config.max_messages as f64;

        HistoryHealth {
            message_count: count,
            max_messages: self.config.max_messages,
            compaction_threshold: self.config.compaction_threshold,
            has_summary: self.summary.is_some(),
            usage_ratio: ratio,
            status: if ratio >= 1.0 {
                HealthStatus::Critical
            } else if ratio >= 0.8 {
                HealthStatus::Warning
            } else {
                HealthStatus::Healthy
            },
        }
    }

    /// 清空历史（用于测试或重置）
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.messages.clear();
        self.summary = None;
    }
}

/// 历史健康状态
#[derive(Debug, Clone)]
pub struct HistoryHealth {
    pub message_count: usize,
    pub max_messages: usize,
    pub compaction_threshold: usize,
    pub has_summary: bool,
    pub usage_ratio: f64,
    pub status: HealthStatus,
}

/// 健康状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    Healthy,
    Warning,
    Critical,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::llm::mock::MockLlmClient;

    #[tokio::test]
    async fn test_history_manager_basic() {
        let config = HistoryConfig {
            max_messages: 10,
            compaction_threshold: 8,
            ..Default::default()
        };
        let mut manager = HistoryManager::new(config);

        // 设置 system prompt
        manager
            .set_system_prompt("You are a helpful assistant")
            .unwrap();

        // 添加消息
        manager.add_user_message("Hello");
        manager.add_assistant_message("Hi there!");
        manager.add_user_message("How are you?");

        assert_eq!(manager.message_count(), 3);
        assert!(!manager.needs_compaction());

        // 构建消息
        let messages = manager.build_messages();
        assert!(messages[0].is_system());
        assert_eq!(messages.len(), 4); // system + 3 messages
    }

    #[tokio::test]
    async fn test_history_manager_trim() {
        let config = HistoryConfig {
            max_messages: 3,
            compaction_threshold: 2,
            ..Default::default()
        };
        let mut manager = HistoryManager::new(config);

        manager.add_user_message("1");
        manager.add_user_message("2");
        manager.add_user_message("3"); // 触发 trim
        manager.add_user_message("4"); // 再次触发 trim

        assert_eq!(manager.message_count(), 3);
    }

    #[tokio::test]
    async fn test_history_manager_compaction() {
        let config = HistoryConfig {
            max_messages: 10,
            compaction_threshold: 5,
            keep_recent_messages: 2,
            ..Default::default()
        };
        let mut manager = HistoryManager::new(config);

        // 添加超过阈值的消息
        for i in 0..6 {
            manager.add_user_message(format!("Message {}", i));
        }

        assert!(manager.needs_compaction());

        // Mock LLM 返回摘要
        let mock = MockLlmClient::with_response("This is a summary of the conversation.");

        // 执行 compaction
        manager.compact(&mock).await.unwrap();

        // compaction 后应该有摘要 + 最近的消息
        assert!(manager.summary.is_some());
        assert_eq!(manager.message_count(), 2); // keep_recent_messages = 2
    }

    #[tokio::test]
    async fn test_health_check() {
        let config = HistoryConfig {
            max_messages: 10,
            compaction_threshold: 8,
            ..Default::default()
        };
        let manager = HistoryManager::new(config);

        let health = manager.health_check();
        assert_eq!(health.status, HealthStatus::Healthy);
        assert_eq!(health.usage_ratio, 0.0);
    }

    #[test]
    fn test_system_prompt_length_limit() {
        let config = HistoryConfig {
            max_system_prompt_len: 10,
            ..Default::default()
        };
        let mut manager = HistoryManager::new(config);

        // 应该成功
        assert!(manager.set_system_prompt("short").is_ok());

        // 应该失败
        assert!(manager.set_system_prompt("this is too long").is_err());
    }
}
