// ============================================================================
// Content Fallback — provider 异常响应兜底解析注册表
// ============================================================================
//
// 部分 LLM provider 在特定条件下不使用标准 OpenAI tool_calls JSON 字段，
// 而是将工具调用信息嵌入 content 字段（如 MiniMax 的 <minimax:tool_call> XML）。
//
// 本模块提供按 provider 注册的 fallback 解析器，tool_loop 在通用解析失败时
// 根据 llm.provider_name() 查询对应 fallback。

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::component::llm::tool_types::ToolCall;

type ContentFallbackFn = fn(&str) -> Option<Vec<ToolCall>>;

/// 全局 fallback 注册表：provider name → parser
static CONTENT_FALLBACKS: OnceLock<HashMap<&'static str, ContentFallbackFn>> = OnceLock::new();

fn fallbacks() -> &'static HashMap<&'static str, ContentFallbackFn> {
    CONTENT_FALLBACKS.get_or_init(|| {
        let mut m = HashMap::new();
        m.insert("minimax", parse_minimax_xml_tool_calls as ContentFallbackFn);
        m
    })
}

/// 尝试用 provider 对应的 fallback 解析器从 content 中提取 tool calls
///
/// 当标准 JSON tool_calls 字段为空时调用，按 provider 查询注册表。
/// 返回 None 表示无匹配 fallback 或解析失败。
pub fn try_parse_content_tool_calls(content: &str, provider: &str) -> Option<Vec<ToolCall>> {
    let table = fallbacks();
    let parser = table.get(provider)?;
    parser(content)
}

// ============================================================================
// MiniMax: <minimax:tool_call> XML 格式解析
// ============================================================================

static MINIMAX_TOOL_ID_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// MiniMax M2.7-highspeed 在 content 中以 XML 而非标准 tool_calls 输出的格式:
/// ```xml
/// <minimax:tool_call>
/// <invoke name="tool_name">
/// <parameter name="param1">value1</parameter>
/// </invoke>
/// </minimax:tool_call>
/// ```
fn parse_minimax_xml_tool_calls(content: &str) -> Option<Vec<ToolCall>> {
    let content = content.trim();
    if !content.starts_with("<minimax:tool_call>") {
        return None;
    }

    let mut calls = Vec::new();
    let search_start = content.find("<minimax:tool_call>")?;
    let end_marker = "</minimax:tool_call>";
    let content_end = content.rfind(end_marker)?;

    let body = &content[search_start..content_end + end_marker.len()];

    let mut pos = 0;
    while pos < body.len() {
        let invoke_start = match body[pos..].find("<invoke ") {
            Some(idx) => pos + idx,
            None => break,
        };

        let tag_end = match body[invoke_start..].find('>') {
            Some(idx) => invoke_start + idx + 1,
            None => break,
        };

        let open_tag = &body[invoke_start..tag_end];
        let tool_name = if let Some(ns) = open_tag.find("name=\"") {
            let after_name = &open_tag[ns + 6..];
            if let Some(quote_end) = after_name.find('"') {
                after_name[..quote_end].to_string()
            } else {
                break;
            }
        } else {
            break;
        };

        let invoke_end = match body[tag_end..].find("</invoke>") {
            Some(idx) => tag_end + idx,
            None => break,
        };

        let params_body = &body[tag_end..invoke_end];
        let mut args = serde_json::Map::new();
        let mut pp = 0;
        while pp < params_body.len() {
            let p_start = match params_body[pp..].find("<parameter name=\"") {
                Some(idx) => pp + idx,
                None => break,
            };
            let p_name_start = p_start + 17; // "<parameter name=\""
            let p_name_end = match params_body[p_name_start..].find('"') {
                Some(idx) => p_name_start + idx,
                None => break,
            };
            let param_name = &params_body[p_name_start..p_name_end];

            let p_value_start = match params_body[p_name_end..].find('>') {
                Some(idx) => p_name_end + idx + 1,
                None => break,
            };
            let p_close = match params_body[p_value_start..].find("</parameter>") {
                Some(idx) => p_value_start + idx,
                None => break,
            };
            let param_value = &params_body[p_value_start..p_close];
            args.insert(
                param_name.to_string(),
                serde_json::Value::String(param_value.to_string()),
            );

            pp = p_close + 12; // "</parameter>"
        }

        let id = format!(
            "minimax_{}",
            MINIMAX_TOOL_ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );

        calls.push(ToolCall {
            id,
            call_type: "function".to_string(),
            function: crate::component::llm::tool_types::ToolCallFunction {
                name: tool_name,
                arguments: serde_json::Value::Object(args).to_string(),
            },
        });

        pos = invoke_end + 9; // "</invoke>"
    }

    if calls.is_empty() { None } else { Some(calls) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimax_single_tool() {
        let xml = r#"<minimax:tool_call>
<invoke name="query_world">
<parameter name="section">environment</parameter>
</invoke>
</minimax:tool_call>"#;
        let calls = parse_minimax_xml_tool_calls(xml).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "query_world");
        let args: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["section"], "environment");
    }

    #[test]
    fn test_parse_minimax_multiple_params() {
        let xml = r#"<minimax:tool_call>
<invoke name="说话">
<parameter name="target_agent_id">abc-123</parameter>
<parameter name="content">你好</parameter>
</invoke>
</minimax:tool_call>"#;
        let calls = parse_minimax_xml_tool_calls(xml).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "说话");
        let args: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["target_agent_id"], "abc-123");
        assert_eq!(args["content"], "你好");
    }

    #[test]
    fn test_parse_minimax_not_xml() {
        assert!(parse_minimax_xml_tool_calls("just some text").is_none());
    }

    #[test]
    fn test_parse_minimax_empty_invoke() {
        let xml = "<minimax:tool_call></minimax:tool_call>";
        assert!(parse_minimax_xml_tool_calls(xml).is_none());
    }

    #[test]
    fn test_fallback_registry_minimax() {
        let xml = r#"<minimax:tool_call>
<invoke name="get_action_detail">
<parameter name="action_name">说话</parameter>
</invoke>
</minimax:tool_call>"#;
        let calls = try_parse_content_tool_calls(xml, "minimax").unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "get_action_detail");
    }

    #[test]
    fn test_fallback_registry_unknown_provider() {
        assert!(
            try_parse_content_tool_calls("<minimax:tool_call></minimax:tool_call>", "openai")
                .is_none()
        );
    }
}
