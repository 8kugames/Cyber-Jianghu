// ============================================================================
// Tool Calling 类型定义
// ============================================================================
//
// OpenAI 兼容的工具调用接口类型。
// 用于 ActorSoul 在决策阶段查询精确的游戏数据 ID。

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// OpenAI 兼容的工具定义（/v1/chat/completions tools 字段）
#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String, // "function"
    pub function: ToolFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolFunction {
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

impl ToolDefinition {
    pub fn new(name: &str, description: &str, parameters: Option<serde_json::Value>) -> Self {
        Self {
            tool_type: "function".to_string(),
            function: ToolFunction {
                name: name.to_string(),
                description: description.to_string(),
                parameters,
            },
        }
    }

    pub fn simple(name: &str, description: &str) -> Self {
        Self::new(
            name,
            description,
            Some(serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            })),
        )
    }
}

/// LLM 返回的 tool call（OpenAI response.choices[].message.tool_calls[]）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_function_type")]
    pub call_type: String,
    pub function: ToolCallFunction,
}

fn default_function_type() -> String {
    "function".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String, // JSON string
}

impl ToolCall {
    pub fn parse_arguments(&self) -> Result<serde_json::Value> {
        if self.function.arguments.is_empty() {
            return Ok(serde_json::json!({}));
        }
        Ok(serde_json::from_str(&self.function.arguments)?)
    }
}

/// 工具执行器 trait
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(&self, name: &str, arguments: &serde_json::Value)
    -> Result<serde_json::Value>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_definition_simple() {
        let def = ToolDefinition::simple("get_inventory", "Query inventory");
        assert_eq!(def.function.name, "get_inventory");
        let serialized = serde_json::to_string(&def).unwrap();
        assert!(serialized.contains("\"type\":\"function\""));
        assert!(serialized.contains("\"name\":\"get_inventory\""));
    }

    #[test]
    fn test_tool_definition_with_params() {
        let def = ToolDefinition::new(
            "use_item",
            "Use an item",
            Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "item_id": {"type": "string", "description": "Item ID"}
                },
                "required": ["item_id"]
            })),
        );
        assert_eq!(def.function.name, "use_item");
        assert!(def.function.parameters.is_some());
    }

    #[test]
    fn test_tool_call_parse_arguments() {
        let tc = ToolCall {
            id: "call_123".to_string(),
            call_type: "function".to_string(),
            function: ToolCallFunction {
                name: "get_inventory".to_string(),
                arguments: r#"{"item_id": "mantou"}"#.to_string(),
            },
        };
        let args = tc.parse_arguments().unwrap();
        assert_eq!(args["item_id"], "mantou");
    }

    #[test]
    fn test_tool_call_parse_empty_arguments() {
        let tc = ToolCall {
            id: "call_456".to_string(),
            call_type: "function".to_string(),
            function: ToolCallFunction {
                name: "get_inventory".to_string(),
                arguments: "".to_string(),
            },
        };
        let args = tc.parse_arguments().unwrap();
        assert_eq!(args, serde_json::json!({}));
    }

    #[test]
    fn test_tool_call_deserialize() {
        let json = r#"{"id":"call_abc","type":"function","function":{"name":"get_inventory","arguments":"{}"}}"#;
        let tc: ToolCall = serde_json::from_str(json).unwrap();
        assert_eq!(tc.id, "call_abc");
        assert_eq!(tc.call_type, "function");
        assert_eq!(tc.function.name, "get_inventory");
    }
}
