// ============================================================================
// OpenClaw Channel 通知器
// ============================================================================
//
// 通过 OpenClaw 的 Tools Invoke API 向用户发送侠客行为日志
//
// 使用场景：
// - 汇报侠客的完整认知链
// - 发送重要事件通知
// - 周期性行为摘要
// ============================================================================

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::core::cognitive::CognitiveChain;

/// OpenClaw Channel 通知器
///
/// 通过 OpenClaw Gateway 的 Tools Invoke API 发送消息到各种 Channel
pub struct OpenClawNotifier {
    /// OpenClaw Gateway 地址
    gateway_url: String,
    /// 认证 Token
    auth_token: String,
    /// 默认 Channel 类型 (discord, whatsapp, telegram 等)
    default_channel: String,
    /// Channel ID (具体频道的 ID)
    channel_id: Option<String>,
}

impl OpenClawNotifier {
    /// 创建新的 OpenClaw 通知器
    pub fn new(
        gateway_url: impl Into<String>,
        auth_token: impl Into<String>,
        default_channel: impl Into<String>,
    ) -> Self {
        Self {
            gateway_url: gateway_url.into(),
            auth_token: auth_token.into(),
            default_channel: default_channel.into(),
            channel_id: None,
        }
    }

    /// 设置 Channel ID
    pub fn with_channel_id(mut self, channel_id: impl Into<String>) -> Self {
        self.channel_id = Some(channel_id.into());
        self
    }

    /// 构建 HTTP 客户端
    fn build_http_client(&self) -> Result<reqwest::Client> {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("Failed to build HTTP client for OpenClaw notifier")
    }

    /// 获取 Tools Invoke 端点 URL
    fn tools_invoke_url(&self) -> String {
        format!("{}/tools/invoke", self.gateway_url.trim_end_matches('/'))
    }

    /// 发送行为日志到 OpenClaw Channel
    pub async fn send_activity_log(&self, log: &ActivityLog) -> Result<()> {
        let client = self.build_http_client()?;
        let url = self.tools_invoke_url();

        // 构建工具调用请求
        let tool_request = ToolInvokeRequest {
            tool: format!("{}_send", self.default_channel),
            arguments: serde_json::json!({
                "channel_id": self.channel_id.as_deref().unwrap_or(&log.channel_id),
                "message": log.message,
            }),
        };

        debug!("Sending activity log to OpenClaw: {}", url);

        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.auth_token))
            .header("Content-Type", "application/json")
            .json(&tool_request)
            .send()
            .await
            .context("Failed to send activity log to OpenClaw")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());
            return Err(anyhow::anyhow!(
                "OpenClaw API error {}: {}",
                status,
                error_body
            ));
        }

        info!("Activity log sent successfully");
        Ok(())
    }

    /// 汇报完整认知链
    pub async fn send_cognitive_chain(&self, chain: &CognitiveChain) -> Result<()> {
        let message = self.format_cognitive_chain(chain);
        let log = ActivityLog {
            channel_id: self.channel_id.clone().unwrap_or_default(),
            message,
            timestamp: chrono::Utc::now(),
            log_type: ActivityLogType::CognitiveChain,
        };

        self.send_activity_log(&log).await
    }

    /// 发送简短的行为摘要
    pub async fn send_summary(&self, chain: &CognitiveChain) -> Result<()> {
        let message = self.format_summary(chain);
        let log = ActivityLog {
            channel_id: self.channel_id.clone().unwrap_or_default(),
            message,
            timestamp: chrono::Utc::now(),
            log_type: ActivityLogType::Summary,
        };

        self.send_activity_log(&log).await
    }

    /// 发送重要事件通知
    pub async fn send_event(&self, event: &str) -> Result<()> {
        let log = ActivityLog {
            channel_id: self.channel_id.clone().unwrap_or_default(),
            message: format!("【事件】{}", event),
            timestamp: chrono::Utc::now(),
            log_type: ActivityLogType::Event,
        };

        self.send_activity_log(&log).await
    }

    // ========================================================================
    // 格式化方法
    // ========================================================================

    /// 格式化完整认知链
    fn format_cognitive_chain(&self, chain: &CognitiveChain) -> String {
        let mut message = format!("【{} 认知链 - Tick {}】\n", chain.agent_name, chain.tick_id);

        for stage_output in &chain.stages {
            message.push_str(&format!(
                "📝 {}:\n{}\n\n",
                stage_output.stage.name(),
                stage_output.content
            ));
        }

        message.push_str(&format!(
            "⚡ 最终决策: {:?}",
            chain.final_intent.action_type
        ));

        message
    }

    /// 格式化行为摘要
    fn format_summary(&self, chain: &CognitiveChain) -> String {
        let perception = chain.get_stage(crate::core::cognitive::CognitiveStage::Perception);
        let motivation = chain.get_stage(crate::core::cognitive::CognitiveStage::Motivation);
        let planning = chain.get_stage(crate::core::cognitive::CognitiveStage::Planning);

        format!(
            "【{} 行为摘要 - Tick {}】\n\
             状态: {}\n\
             动机: {}\n\
             计划: {}\n\
             决策: {:?}",
            chain.agent_name,
            chain.tick_id,
            perception.map(|s| s.content.as_str()).unwrap_or("(无)"),
            motivation.map(|s| s.content.as_str()).unwrap_or("(无)"),
            planning.map(|s| s.content.as_str()).unwrap_or("(无)"),
            chain.final_intent.action_type
        )
    }
}

// ============================================================================
// 类型定义
// ============================================================================

/// 行为日志
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityLog {
    /// Channel ID
    pub channel_id: String,
    /// 消息内容
    pub message: String,
    /// 时间戳
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// 日志类型
    pub log_type: ActivityLogType,
}

/// 日志类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActivityLogType {
    /// 完整认知链
    CognitiveChain,
    /// 行为摘要
    Summary,
    /// 重要事件
    Event,
}

/// Tools Invoke 请求
#[derive(Debug, Serialize)]
struct ToolInvokeRequest {
    tool: String,
    arguments: serde_json::Value,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_notifier_creation() {
        let notifier = OpenClawNotifier::new("http://localhost:23333", "test-token", "discord");

        assert_eq!(notifier.gateway_url, "http://localhost:23333");
        assert_eq!(notifier.auth_token, "test-token");
        assert_eq!(notifier.default_channel, "discord");
        assert!(notifier.channel_id.is_none());
    }

    #[test]
    fn test_notifier_with_channel_id() {
        let notifier = OpenClawNotifier::new("http://localhost:23333", "test-token", "discord")
            .with_channel_id("test-channel-123");

        assert_eq!(notifier.channel_id, Some("test-channel-123".to_string()));
    }

    #[test]
    fn test_tools_invoke_url() {
        let notifier = OpenClawNotifier::new("http://localhost:23333/", "test-token", "discord");

        assert_eq!(
            notifier.tools_invoke_url(),
            "http://localhost:23333/tools/invoke"
        );

        let notifier = OpenClawNotifier::new("http://localhost:23333", "test-token", "discord");

        assert_eq!(
            notifier.tools_invoke_url(),
            "http://localhost:23333/tools/invoke"
        );
    }

    #[test]
    fn test_activity_log_serialization() {
        let log = ActivityLog {
            channel_id: "test-channel".to_string(),
            message: "测试消息".to_string(),
            timestamp: chrono::Utc.with_ymd_and_hms(2024, 1, 1, 12, 0, 0).unwrap(),
            log_type: ActivityLogType::Event,
        };

        let json = serde_json::to_string(&log).unwrap();
        assert!(json.contains("测试消息"));
        assert!(json.contains("Event"));
    }

    #[test]
    fn test_tool_invoke_request_serialization() {
        let request = ToolInvokeRequest {
            tool: "discord_send".to_string(),
            arguments: serde_json::json!({
                "channel_id": "test",
                "message": "Hello"
            }),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("discord_send"));
        assert!(json.contains("Hello"));
    }
}
