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

    pub fn canonical_json(&self) -> String {
        use super::canonicalize::canonicalize_json_schema;
        let mut value = serde_json::to_value(self).expect("ToolDefinition serializes");
        canonicalize_json_schema(&mut value);
        serde_json::to_string(&value).expect("canonical value serializes")
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
        let normalized = super::client::normalize_double_braces(&self.function.arguments);
        match serde_json::from_str(normalized.as_ref()) {
            Ok(v) => Ok(v),
            Err(first_err) => {
                tracing::warn!(
                    "Tool arguments parse failed, raw preview: {}",
                    self.function
                        .arguments
                        .chars()
                        .take(200)
                        .collect::<String>()
                );
                Err(first_err.into())
            }
        }
    }
}

/// 流式 SSE tool_calls 增量 chunk
///
/// 首个 chunk: `{id: "call_xxx", index: 0, type: "function", function: {name: "skill_view", arguments: ""}}`
/// 后续 chunk: `{id: null, index: 0, type: "function", function: {name: "", arguments: "fragment"}}`
#[derive(Debug, Clone, Deserialize)]
pub struct StreamToolCallDelta {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub index: u32,
    #[serde(rename = "type", default)]
    pub call_type: Option<String>,
    #[serde(default)]
    pub function: StreamToolCallFunctionDelta,
}

/// 流式 tool_calls function 增量
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StreamToolCallFunctionDelta {
    #[serde(default, deserialize_with = "deserialize_null_as_default")]
    pub name: String,
    #[serde(default, deserialize_with = "deserialize_null_as_default")]
    pub arguments: String,
}

/// serde 反序列化：将 `null` 视为类型的 Default 值（对 String 即 ""）
fn deserialize_null_as_default<'de, D, T>(de: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de> + Default,
{
    Option::<T>::deserialize(de).map(|v| v.unwrap_or_default())
}

/// 流式 tool_calls 累积器 — 按 index 合并增量 chunk 为完整 ToolCall
pub struct StreamToolCallAccumulator {
    calls: std::collections::HashMap<u32, (String, String, String)>, // index → (id, name, arguments)
}

impl StreamToolCallAccumulator {
    pub fn new() -> Self {
        Self {
            calls: std::collections::HashMap::new(),
        }
    }

    /// 追加一个增量 chunk
    pub fn push(&mut self, delta: &StreamToolCallDelta) {
        let entry = self.calls.entry(delta.index).or_default();
        if let Some(ref id) = delta.id
            && !id.is_empty()
        {
            entry.0 = id.clone();
        }
        if !delta.function.name.is_empty() {
            entry.1 = delta.function.name.clone();
        }
        entry.2.push_str(&delta.function.arguments);
    }

    /// 消费累积器，返回完整的 ToolCall 列表（按 index 排序）
    pub fn into_tool_calls(self) -> Vec<ToolCall> {
        let mut indexed: Vec<(u32, (String, String, String))> = self.calls.into_iter().collect();
        indexed.sort_by_key(|(i, _)| *i);
        indexed
            .into_iter()
            .map(|(_, (id, name, arguments))| ToolCall {
                id,
                call_type: "function".to_string(),
                function: ToolCallFunction { name, arguments },
            })
            .collect()
    }
}

impl Default for StreamToolCallAccumulator {
    fn default() -> Self {
        Self::new()
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
                arguments: r#"{"item_id": "馒头"}"#.to_string(),
            },
        };
        let args = tc.parse_arguments().unwrap();
        assert_eq!(args["item_id"], "馒头");
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

    #[test]
    fn test_stream_function_delta_null_fields() {
        // LongCat 首个 tool_call chunk 发送 arguments: null
        let json = r#"{"name": "search_memory", "arguments": null}"#;
        let delta: StreamToolCallFunctionDelta = serde_json::from_str(json).unwrap();
        assert_eq!(delta.name, "search_memory");
        assert_eq!(delta.arguments, "");
    }

    #[test]
    fn canonical_json_is_byte_stable_across_calls() {
        let tool = ToolDefinition::new(
            "test_fn",
            "test",
            Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "z_param": {"type": "string"},
                    "a_param": {"type": "string"},
                },
                "required": ["z_param", "a_param"],
            })),
        );
        let json1 = tool.canonical_json();
        let json2 = tool.canonical_json();
        assert_eq!(
            json1, json2,
            "canonical_json must be byte-identical across calls"
        );
        assert!(json1.contains("\"a_param\":{\"type\":\"string\"}"));
        assert!(json1.contains("\"properties\":{\"a_param\":"));
        assert!(json1.contains("\"required\":[\"a_param\",\"z_param\"]"));
    }

    #[test]
    fn test_stream_function_delta_both_null() {
        let json = r#"{"name": null, "arguments": null}"#;
        let delta: StreamToolCallFunctionDelta = serde_json::from_str(json).unwrap();
        assert_eq!(delta.name, "");
        assert_eq!(delta.arguments, "");
    }

    #[test]
    fn test_stream_accumulator_null_then_content() {
        let mut acc = StreamToolCallAccumulator::new();
        // 首个 chunk: arguments=null → ""
        acc.push(&StreamToolCallDelta {
            id: Some("call_1".to_string()),
            index: 0,
            call_type: Some("function".to_string()),
            function: StreamToolCallFunctionDelta {
                name: "get_action_detail".to_string(),
                arguments: String::new(), // null → ""
            },
        });
        // 后续 chunk: arguments 片段
        acc.push(&StreamToolCallDelta {
            id: None,
            index: 0,
            call_type: None,
            function: StreamToolCallFunctionDelta {
                name: String::new(),
                arguments: r#"{"action_type":"取"}"#.to_string(),
            },
        });
        let calls = acc.into_tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].function.name, "get_action_detail");
        assert_eq!(calls[0].function.arguments, r#"{"action_type":"取"}"#);
    }
}
