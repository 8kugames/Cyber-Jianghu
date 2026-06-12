//! 模型特化适配层
//!
//! 处理不同 LLM 提供商的响应格式差异，将模型专有格式转换为通用格式。
//! 下游 JSON 提取管线只处理已规范化的通用内容。

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;

// ── 标准 XML 标签 (think, reasoning 等) ──

static PAIRED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?is)<(?:think_tag|think|reasoning|thought|thinking|minimax:tool_call|invoke)[^>]*>.*?</(?:think_tag|think|reasoning|thought|thinking|minimax:tool_call|invoke)[^>]*>"
    ).expect("paired tag regex valid")
});

static SELF_CLOSING_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)<(?:think_tag|think|reasoning|thought|thinking|minimax:tool_call|invoke)[^>]*/>\s*")
        .expect("self-closing tag regex valid")
});

static OPENING_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)<(?:think_tag|think|reasoning|thought|thinking|minimax:tool_call|invoke)[^>]*>")
        .expect("opening tag regex valid")
});

static CLOSING_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)</(?:think_tag|think|reasoning|thought|thinking|minimax:tool_call|invoke)[^>]*>\s*")
        .expect("closing tag regex valid")
});

// ── DeepSeek DSML 标签 (全角竖线 ｜ U+FF5C) ──
// 格式: <｜｜DSML｜｜tag_name>...</｜｜DSML｜｜tag_name>
// 也兼容半角竖线 | 的变体

/// DSML 标签对（含内容移除）：匹配完整 DSML 块
static DSML_PAIRED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)</?[｜|]{2}DSML[｜|]{2}[^>]*>.*?</?[｜|]{2}DSML[｜|]{2}[^>]*>"#)
        .expect("DSML paired tag regex valid")
});

/// DSML 单个标签：`<｜｜DSML｜｜...>` 或 `</｜｜DSML｜｜...>`
static DSML_TAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)</?[｜|]{2}DSML[｜|]{2}[^>]*>\s*"#).expect("DSML single tag regex valid")
});

/// DSML 标签的快速检测子串（全角版本 — 实际输出格式）
const DSML_MARKER: &str = "\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}";

const TAG_NAMES: &[&str] = &[
    "think_tag",
    "think",
    "reasoning",
    "thought",
    "thinking",
    "minimax:tool_call",
    "invoke",
];

/// 规范化 LLM 响应内容中的模型专有格式
///
/// 策略：
/// 1. DeepSeek DSML 标签剥离（全角竖线格式，优先处理）
/// 2. 标准 thinking 标签剥离
/// 3. 若标签移除后为空，尝试保留标签内内容
pub(crate) fn normalize_llm_content(content: &str) -> Cow<'_, str> {
    if !content.contains('<') {
        return Cow::Borrowed(content);
    }

    // Phase 1: DeepSeek DSML 标签剥离
    let after_dsml = strip_dsml_tags(content);
    let content = match &after_dsml {
        Cow::Owned(s) => s.as_str(),
        Cow::Borrowed(_) => return handle_standard_tags(content),
    };

    // DSML 剥离后，继续检查标准标签
    match handle_standard_tags(content) {
        Cow::Owned(s) => Cow::Owned(s),
        Cow::Borrowed(_) => after_dsml,
    }
}

/// 处理标准 thinking/reasoning 标签
fn handle_standard_tags(content: &str) -> Cow<'_, str> {
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

/// 剥离 DeepSeek DSML 标签（<｜｜DSML｜｜...> 格式）
///
/// DSML 标签是 DeepSeek v4 在 tool calling 时输出的非标准格式。
/// 标签使用全角竖线 ｜ (U+FF5C) 作为分隔符。
fn strip_dsml_tags(content: &str) -> Cow<'_, str> {
    // 快速路径：不含 DSML 标记
    if !content.contains(DSML_MARKER) && !content.contains("||DSML||") {
        return Cow::Borrowed(content);
    }

    // 先尝试移除 DSML 标签对及其内容
    let stripped = DSML_PAIRED_RE.replace_all(content, "");
    let result = DSML_TAG_RE.replace_all(&stripped, "");

    if result.trim().is_empty() {
        // 移除标签后内容为空 — 尝试仅移除标签保留内容
        let preserved = DSML_TAG_RE.replace_all(content, "");
        if preserved != content {
            return Cow::Owned(preserved.into_owned());
        }
        return Cow::Borrowed(content);
    }

    if result == content {
        return Cow::Borrowed(content);
    }

    Cow::Owned(result.into_owned())
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

    // ── DeepSeek DSML 标签测试 ──

    #[test]
    fn dsml_tool_calls_stripped() {
        // 实际 DeepSeek 输出格式 (全角竖线 U+FF5C)
        let input = "<\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}tool_calls><\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}invoke name=\"query_world\"><\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}parameter name=\"section\" string=\"true\">location</\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}parameter></\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}invoke></\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}tool_calls>{\"actions\":[{\"action_type\":\"说话\"}]}";
        let result = normalize_llm_content(input);
        assert_eq!(result.as_ref(), r#"{"actions":[{"action_type":"说话"}]}"#);
    }

    #[test]
    fn dsml_only_returns_empty_stripped() {
        // 纯 DSML 内容, 无 JSON — 移除后为空
        let input = "<\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}tool_calls>some content</\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}tool_calls>";
        let result = normalize_llm_content(input);
        // 策略 2: 保留标签内内容
        assert_eq!(result.as_ref(), "some content");
    }

    #[test]
    fn dsml_halfwidth_pipe_compatible() {
        // 半角竖线变体 (以防万一)
        let input = r#"<||DSML||tool_calls>reasoning</||DSML||tool_calls>{"x":1}"#;
        let result = normalize_llm_content(input);
        assert_eq!(result.as_ref(), r#"{"x":1}"#);
    }

    #[test]
    fn dsml_closing_tag_before_json() {
        // 闭标签出现在 JSON 前 (实际错误场景)
        let input = "</\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}tool_calls>{\"result\":true}";
        let result = normalize_llm_content(input);
        assert_eq!(result.as_ref(), r#"{"result":true}"#);
    }

    #[test]
    fn dsml_mixed_with_think_tag() {
        // DSML + think 标签混合
        let input = "<\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}tool_calls>dsml content</\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}tool_calls><think_tag>reasoning</think_tag>{\"x\":1}";
        let result = normalize_llm_content(input);
        assert_eq!(result.as_ref(), r#"{"x":1}"#);
    }

    #[test]
    fn no_dsml_returns_borrowed() {
        assert!(matches!(
            normalize_llm_content("plain text without tags"),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn invoke_tag_stripped_before_json() {
        // LLM 输出 <invoke> 包裹的内容后跟 JSON
        let input = r#"<invoke name="query_world">{"section":"state"}</invoke>{"actions":[{"action_type":"说话"}]}"#;
        let result = normalize_llm_content(input);
        assert_eq!(result.as_ref(), r#"{"actions":[{"action_type":"说话"}]}"#);
    }

    #[test]
    fn invoke_tag_only_preserves_content() {
        // JSON 被包裹在 invoke 标签内（罕见情况）
        let input = r#"<invoke>{"actions":[{"action_type":"说话"}]}</invoke>"#;
        let result = normalize_llm_content(input);
        assert_eq!(result.as_ref(), r#"{"actions":[{"action_type":"说话"}]}"#);
    }
}
