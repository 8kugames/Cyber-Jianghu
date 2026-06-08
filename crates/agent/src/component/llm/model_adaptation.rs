//! 模型特化适配层
//!
//! 处理不同 LLM 提供商的响应格式差异，将模型专有格式转换为通用格式。
//! 下游 JSON 提取管线只处理已规范化的通用内容。

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;

static PAIRED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?is)<(?:think_tag|think|reasoning|thought|thinking|minimax:tool_call)[^>]*>.*?</(?:think_tag|think|reasoning|thought|thinking|minimax:tool_call)[^>]*>"
    ).expect("paired tag regex valid")
});

static SELF_CLOSING_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)<(?:think_tag|think|reasoning|thought|thinking|minimax:tool_call)[^>]*/>\s*")
        .expect("self-closing tag regex valid")
});

static OPENING_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)<(?:think_tag|think|reasoning|thought|thinking|minimax:tool_call)[^>]*>")
        .expect("opening tag regex valid")
});

static CLOSING_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)</(?:think_tag|think|reasoning|thought|thinking|minimax:tool_call)[^>]*>\s*")
        .expect("closing tag regex valid")
});

const TAG_NAMES: &[&str] = &[
    "think_tag",
    "think",
    "reasoning",
    "thought",
    "thinking",
    "minimax:tool_call",
];

/// 规范化 LLM 响应内容中的模型专有格式
///
/// 策略：
/// 1. 先尝试移除标签及其内容（常见情况：JSON 在标签之后）
/// 2. 若结果为空，则仅移除标签保留内容（罕见情况：JSON 被包裹在标签内）
pub(crate) fn normalize_llm_content(content: &str) -> Cow<'_, str> {
    if !content.contains('<') {
        return Cow::Borrowed(content);
    }

    let has_any_tag = TAG_NAMES
        .iter()
        .any(|tag| content.contains(&format!("<{}", tag)));
    if !has_any_tag {
        return Cow::Borrowed(content);
    }

    // 策略 1：移除标签对及内容（常见情况：JSON 在标签之后）
    let stripped = strip_tag_pairs_with_content(content);
    if !stripped.trim().is_empty() {
        return Cow::Owned(stripped);
    }

    // 策略 2：仅移除标签标记，保留内容（JSON 被包裹在标签内的罕见情况）
    let preserved = strip_tags_preserve_content(content);
    if preserved != content {
        return Cow::Owned(preserved);
    }

    Cow::Borrowed(content)
}

/// 移除 thinking 标签对及其全部内容
fn strip_tag_pairs_with_content(content: &str) -> String {
    let result = PAIRED_RE.replace_all(content, "");
    SELF_CLOSING_RE.replace_all(&result, "").to_string()
}

/// 仅移除标签标记，保留标签间内容
fn strip_tags_preserve_content(content: &str) -> String {
    let result = OPENING_RE.replace_all(content, "");
    CLOSING_RE.replace_all(&result, "").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_tags_returns_borrowed() {
        assert!(matches!(
            normalize_llm_content("plain text"),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn json_after_think_tag() {
        let input = r#"<think_tag>some reasoning</think_tag>{"action":"move"}"#;
        let result = normalize_llm_content(input);
        assert_eq!(result.as_ref(), r#"{"action":"move"}"#);
    }

    #[test]
    fn json_inside_think_tag() {
        let input = r#"<think_tag>{"action":"move"}</think_tag>"#;
        let result = normalize_llm_content(input);
        assert_eq!(result.as_ref(), r#"{"action":"move"}"#);
    }

    #[test]
    fn json_mixed_inside_think_tag() {
        let input = r#"<think_tag>reasoning {"action":"move"}</think_tag>"#;
        let result = normalize_llm_content(input);
        assert_eq!(result.as_ref(), r#"reasoning {"action":"move"}"#);
    }

    #[test]
    fn deepseek_think_tag() {
        let input = r#"<think length="123">reasoning here</think >{"result":true}"#;
        let result = normalize_llm_content(input);
        assert_eq!(result.as_ref(), r#"{"result":true}"#);
    }

    #[test]
    fn self_closing_tag() {
        let input = r#"<think />{"action":"move"}"#;
        let result = normalize_llm_content(input);
        assert_eq!(result.as_ref(), r#"{"action":"move"}"#);
    }

    #[test]
    fn reasoning_tag() {
        let input = r#"<reasoning>thought process</reasoning>{"x":1}"#;
        let result = normalize_llm_content(input);
        assert_eq!(result.as_ref(), r#"{"x":1}"#);
    }

    #[test]
    fn thinking_tag() {
        let input = r#"<thinking>deep thought</thinking>{"answer":42}"#;
        let result = normalize_llm_content(input);
        assert_eq!(result.as_ref(), r#"{"answer":42}"#);
    }

    #[test]
    fn json_inside_thinking_tag() {
        let input = r#"<thinking>{"answer":42}</thinking>"#;
        let result = normalize_llm_content(input);
        assert_eq!(result.as_ref(), r#"{"answer":42}"#);
    }

    #[test]
    fn multiple_sequential_blocks() {
        let input = r#"<think_tag>a</think_tag><reasoning>b</reasoning>{"x":1}"#;
        let result = normalize_llm_content(input);
        assert_eq!(result.as_ref(), r#"{"x":1}"#);
    }

    #[test]
    fn unrelated_tags_ignored() {
        let input = r#"<div>content</div>{"x":1}"#;
        let result = normalize_llm_content(input);
        assert!(matches!(result, Cow::Borrowed(_)));
    }
}
