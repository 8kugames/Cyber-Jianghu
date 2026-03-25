// Turn Cycle - Agent 决策循环
//
// 参考 ZeroClaw 的 run_tool_call_loop() 设计
//
// Turn Cycle 是 Agent 的核心决策循环：
// 1. 接收 WorldState
// 2. 构建 Context
// 3. 调用 LLM（可能多次，带 tool calls）
// 4. 返回最终 Intent
//
// 超时控制：最大迭代次数和总超时时间

use anyhow::{Context, Result};
use tracing::debug;

use crate::models::WorldState;

use super::history::HistoryManager;

// ============================================================================
// 常量
// ============================================================================

const DEFAULT_MAX_ITERATIONS: usize = 10;
const DEFAULT_TIMEOUT_SECS: u64 = 300;

// ============================================================================
// 配置
// ============================================================================

#[derive(Debug, Clone)]
pub struct TurnCycleConfig {
    pub max_iterations: usize,
    pub timeout_secs: u64,
}

impl Default for TurnCycleConfig {
    fn default() -> Self {
        Self {
            max_iterations: DEFAULT_MAX_ITERATIONS,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }
}

// ============================================================================
// Tool Call
// ============================================================================

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub id: String,
    pub name: String,
    pub result: String,
    pub error: Option<String>,
}

impl ToolResult {
    pub fn success(id: impl Into<String>, name: impl Into<String>, result: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            result: result.into(),
            error: None,
        }
    }

    pub fn failure(id: impl Into<String>, name: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            result: String::new(),
            error: Some(error.into()),
        }
    }
}

// ============================================================================
// Intent 结果
// ============================================================================

#[derive(Debug, Clone)]
pub struct Intent {
    pub action_type: String,
    pub action_data: Option<serde_json::Value>,
    pub thought: String,
}

// ============================================================================
// Turn Cycle
// ============================================================================

pub struct TurnCycle {
    config: TurnCycleConfig,
}

impl TurnCycle {
    pub fn new(config: TurnCycleConfig) -> Self {
        Self { config }
    }

    pub async fn run<S: TurnCycleServices>(
        &self,
        services: &S,
        _world_state: &WorldState,
        history: &mut HistoryManager,
    ) -> Result<Intent> {
        let start_time = std::time::Instant::now();
        let mut iterations = 0;

        loop {
            iterations += 1;

            if iterations > self.config.max_iterations {
                anyhow::bail!("Max iterations {} exceeded", self.config.max_iterations);
            }

            if start_time.elapsed().as_secs() > self.config.timeout_secs {
                anyhow::bail!("Timeout {}s exceeded", self.config.timeout_secs);
            }

            debug!("Turn cycle iteration {}/{}", iterations, self.config.max_iterations);

            let response = services.call_llm(history).await?;

            if let Some(tool_calls) = self.parse_tool_calls(&response) {
                for call in tool_calls {
                    let result = services.execute_tool(&call).await;
                    let tool_result = match result {
                        Ok(r) => ToolResult::success(&call.id, &call.name, &r),
                        Err(e) => {
                            tracing::warn!("Tool {} failed: {}", call.name, e);
                            ToolResult::failure(&call.id, &call.name, e.to_string())
                        }
                    };
                    let result_str = if let Some(ref err) = tool_result.error {
                        format!("error: {}", err)
                    } else {
                        tool_result.result.clone()
                    };
                    history.add_tool_message(&call.id, &call.name, &result_str);
                }
            } else {
                return self.parse_intent(&response);
            }
        }
    }

    fn parse_tool_calls(&self, response: &str) -> Option<Vec<ToolCall>> {
        // 尝试解析 JSON 格式的 tool calls
        // 格式: {"tool_calls": [{"id": "1", "name": "tool_name", "arguments": {...}}]}
        let json: serde_json::Value = serde_json::from_str(response).ok()?;

        let tool_calls = json.get("tool_calls")?.as_array()?;

        let mut calls = Vec::new();
        for item in tool_calls {
            let id = item.get("id")?.as_str()?.to_string();
            let name = item.get("name")?.as_str()?.to_string();
            let arguments = item.get("arguments").cloned().unwrap_or(serde_json::json!({}));

            calls.push(ToolCall { id, name, arguments });
        }

        if calls.is_empty() {
            return None;
        }

        Some(calls)
    }

    fn parse_intent(&self, response: &str) -> Result<Intent> {
        let json: serde_json::Value = serde_json::from_str(response)
            .context("Failed to parse intent JSON")?;

        let action_type = json
            .get("action_type")
            .and_then(|v| v.as_str())
            .unwrap_or("idle")
            .to_string();

        let action_data = json.get("action_data").cloned();
        let thought = json
            .get("thought")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        Ok(Intent {
            action_type,
            action_data,
            thought,
        })
    }
}

impl Default for TurnCycle {
    fn default() -> Self {
        Self::new(TurnCycleConfig::default())
    }
}

// ============================================================================
// Turn Cycle Services Trait
// ============================================================================

#[async_trait::async_trait]
pub trait TurnCycleServices: Send + Sync {
    async fn call_llm(&self, history: &HistoryManager) -> Result<String>;
    async fn execute_tool(&self, call: &ToolCall) -> Result<String>;
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::llm::mock::MockLlmClient;

    struct MockServices {
        responses: Vec<String>,
        current: std::sync::Mutex<usize>,
    }

    impl MockServices {
        fn new(responses: Vec<String>) -> Self {
            Self {
                responses,
                current: std::sync::Mutex::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl TurnCycleServices for MockServices {
        async fn call_llm(&self, _history: &HistoryManager) -> Result<String> {
            let mut idx = self.current.lock().unwrap();
            if *idx < self.responses.len() {
                let response = self.responses[*idx].clone();
                *idx += 1;
                Ok(response)
            } else {
                anyhow::bail!("No more responses")
            }
        }

        async fn execute_tool(&self, _call: &ToolCall) -> Result<String> {
            Ok("tool result".to_string())
        }
    }

    #[tokio::test]
    async fn test_single_response_intent() {
        let turn_cycle = TurnCycle::default();
        let history = HistoryManager::default();

        let services = MockServices::new(vec![r#"{
            "action_type": "move",
            "action_data": {"target": "north"},
            "thought": "I should move north"
        }"#.to_string()]);

        let ws = WorldState::default();
        let intent = turn_cycle.run(&services, &ws, &mut history.clone()).await.unwrap();

        assert_eq!(intent.action_type, "move");
    }

    #[tokio::test]
    async fn test_tool_call_loop() {
        let turn_cycle = TurnCycle::default();
        let history = HistoryManager::default();

        let responses = vec![
            r#"{"tool_calls": [{"id": "1", "name": "search", "arguments": {"query": "test"}}]}"#.to_string(),
            r#"{"action_type": "idle", "thought": "done"}"#.to_string(),
        ];

        let services = MockServices::new(responses);
        let ws = WorldState::default();
        let intent = turn_cycle.run(&services, &ws, &mut history.clone()).await.unwrap();

        assert_eq!(intent.action_type, "idle");
    }

    #[tokio::test]
    async fn test_max_iterations_exceeded() {
        let config = TurnCycleConfig {
            max_iterations: 2,
            timeout_secs: 300,
        };
        let turn_cycle = TurnCycle::new(config);
        let history = HistoryManager::default();

        let responses = vec![
            r#"{"tool_calls": [{"id": "1", "name": "a", "arguments": {}}]}"#.to_string(),
            r#"{"tool_calls": [{"id": "2", "name": "b", "arguments": {}}]}"#.to_string(),
            r#"{"tool_calls": [{"id": "3", "name": "c", "arguments": {}}]}"#.to_string(),
        ];

        let services = MockServices::new(responses);
        let ws = WorldState::default();
        let result = turn_cycle.run(&services, &ws, &mut history.clone()).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Max iterations"));
    }
}
