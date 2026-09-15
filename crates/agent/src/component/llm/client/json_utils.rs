// ============================================================================
// LLM 输出 JSON 提取与修复（纯函数）
// ============================================================================
//
// 职责：从 LLM 原始输出中稳健地提取 JSON——
// 双花括号归一、代码围栏剥离、括号深度扫描截取、常见残缺修复、
// 以及面向截断重试的 truncation 判定。
// 被 client（LlmClientExt 默认实现）、direct_client、tool_types 共用。

use anyhow::Result;
use serde::de::DeserializeOwned;

/// Normalize LLM 输出中错误转义的双花括号
///
/// 部分 LLM（如 LongCat-2.0-Preview）将 JSON 结构中的 `{` 输出为 `{{`。
/// 在本项目领域（游戏动作 JSON）中，字符串值内不会出现 `{{`，因此全局替换安全。
pub(crate) fn normalize_double_braces(s: &str) -> std::borrow::Cow<'_, str> {
    if s.contains("{{") || s.contains("}}") {
        std::borrow::Cow::Owned(s.replace("{{", "{").replace("}}", "}"))
    } else {
        std::borrow::Cow::Borrowed(s)
    }
}

/// 从 LLM 响应中提取 JSON 字符串
///
/// 使用大括号计数找第一个完整 JSON 对象，避免 LLM 在 JSON 后输出额外内容
/// 导致 "trailing characters" 解析错误（如 MiniMax 输出多行 JSON）。
pub(super) fn extract_json_str(response: &str) -> std::borrow::Cow<'_, str> {
    let normalized = normalize_double_braces(response);
    let response = normalized.as_ref();

    if let Some(start) = response.find("```json") {
        let after_marker = start + 7;
        if let Some(end) = response[after_marker..].find("```") {
            std::borrow::Cow::Owned(
                response[after_marker..after_marker + end]
                    .trim()
                    .to_string(),
            )
        } else {
            std::borrow::Cow::Owned(response[after_marker..].trim().to_string())
        }
    } else if let Some(end) = find_first_json_end(response) {
        std::borrow::Cow::Owned(response[..=end].to_string())
    } else {
        std::borrow::Cow::Owned(response.trim().to_string())
    }
}

/// 用大括号计数找第一个完整 JSON 对象的结束位置
///
/// 从第一个 `{` 开始，逐字符追踪大括号深度（跳过字符串内的大括号），
/// 当深度归零时即为第一个完整 JSON 对象的末尾。
pub(super) fn find_first_json_end(s: &str) -> Option<usize> {
    let start = s.find('{')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;

    for (i, &c) in s.as_bytes().iter().enumerate().skip(start) {
        if escape {
            escape = false;
            continue;
        }
        if c == b'\\' && in_string {
            escape = true;
            continue;
        }
        if c == b'"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        if c == b'{' {
            depth += 1;
        } else if c == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// LLM JSON 单遍结构化修复
///
/// 合并归一化 + 括号平衡为单遍状态机，消除两阶段状态漂移。
///
/// 核心原理：JSON 语法是确定性的——字符串闭合引号后**必须**跟随结构字符
/// （, } ] :）或空白+结构字符。不满足此条件的引号为字符串内嵌内容 → 转义。
///
/// 处理范围（单遍完成）：
/// 1. 中文引号 "" → " （字符串外）
/// 2. 单行注释 // ... → 移除
/// 3. 字符串内嵌未转义 " → 转义为 \"（key/value 上下文分别判定）
/// 4. 括号 {}[] 深度追踪 + 自动补全
/// 5. 尾部逗号/引号清理
///
/// 引号闭合判定规则（JSON 语法确定性）：
/// - key 闭合引号后：, } ] : 均合法（key 总是跟 : 配对）
/// - value 闭合引号后：仅 , } ] 合法（value 不跟 :）
///   通过 expect_key/in_key 追踪 key vs value 上下文。
///
/// 已知限制：无法区分「key 缺失闭合引号」和「值内嵌引号」，
/// 如 "appearance: "text" 会被整体视为一个 key 字符串。
pub(super) fn repair_llm_json(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut result = String::with_capacity(input.len() + 32);
    let mut i = 0;

    // 状态
    let mut in_string = false;
    // key vs value 上下文：{ 或 , 后期望 key，: 后期望 value
    let mut expect_key = false;
    let mut in_key = false;
    // 括号栈：追踪 {} [] 嵌套（同时用于平衡补全）
    let mut bracket_stack: Vec<char> = Vec::new();

    while i < len {
        let c = chars[i];

        if in_string {
            // 转义序列：原样透传
            if c == '\\' && i + 1 < len {
                result.push('\\');
                i += 1;
                result.push(chars[i]);
                i += 1;
                continue;
            }

            // 遇到 " 或中文右引号：判断是字符串闭合还是值内未转义引号
            if c == '"' || c == '\u{201d}' {
                // 跳过空白，找到下一个有意义的字符
                let mut j = i + 1;
                while j < len && matches!(chars[j], ' ' | '\t' | '\n' | '\r') {
                    j += 1;
                }
                let next_meaningful = if j < len { Some(chars[j]) } else { None };

                // 闭合判定规则（基于 JSON 语法确定性规则）：
                // - key 闭合引号后必须跟 : → , } ] : 均合法
                // - value 闭合引号后必须跟 , } ] → 仅 , } ] 合法
                let is_closing = if in_key {
                    next_meaningful.is_none_or(|ch| matches!(ch, ',' | '}' | ']' | ':'))
                } else {
                    next_meaningful.is_none_or(|ch| matches!(ch, ',' | '}' | ']'))
                };

                if is_closing {
                    in_string = false;
                    in_key = false;
                    result.push('"');
                } else {
                    // 值内未转义引号 → 转义
                    result.push_str("\\\"");
                }
                i += 1;
                continue;
            }

            result.push(c);
            i += 1;
            continue;
        }

        // === 结构上下文 ===
        match c {
            '"' => {
                in_string = true;
                in_key = expect_key;
                result.push('"');
            }
            '\u{201c}' => {
                // 中文左引号 → ASCII 开引号
                in_string = true;
                in_key = expect_key;
                result.push('"');
            }
            '\u{201d}' => {
                // 中文右引号 → ASCII 闭引号
                in_string = false;
                in_key = false;
                result.push('"');
            }
            '{' => {
                bracket_stack.push('{');
                expect_key = true;
                result.push('{');
            }
            '}' => {
                // 移除尾逗号：, } → }
                if result.ends_with(',') {
                    result.pop();
                    result = result.trim_end().to_string();
                }
                // 括号平衡：弹出到匹配的 {
                if bracket_stack.is_empty() {
                    i += 1;
                    continue;
                }
                while let Some(&top) = bracket_stack.last() {
                    if top == '{' {
                        bracket_stack.pop();
                        break;
                    }
                    bracket_stack.pop();
                    result.push(']');
                }
                result.push('}');
                expect_key = false;

                // }, { 模式：连续对象间自动补全中间缺失的 }
                let mut peek = i + 1;
                while peek < len && matches!(chars[peek], ' ' | '\t' | '\n' | '\r') {
                    peek += 1;
                }
                if peek < len && chars[peek] == ',' {
                    peek += 1;
                    while peek < len && matches!(chars[peek], ' ' | '\t' | '\n' | '\r') {
                        peek += 1;
                    }
                    if peek < len && chars[peek] == '{' {
                        while bracket_stack.last() == Some(&'{') {
                            bracket_stack.pop();
                            result.push('}');
                        }
                    }
                }
            }
            '[' => {
                bracket_stack.push('[');
                expect_key = false;
                result.push('[');
            }
            ']' => {
                // 移除尾逗号：, ] → ]
                if result.ends_with(',') {
                    result.pop();
                    result = result.trim_end().to_string();
                }
                if bracket_stack.is_empty() {
                    i += 1;
                    continue;
                }
                while let Some(&top) = bracket_stack.last() {
                    if top == '[' {
                        bracket_stack.pop();
                        break;
                    }
                    bracket_stack.pop();
                    result.push('}');
                }
                result.push(']');
            }
            ':' => {
                expect_key = false;
                result.push(':');
            }
            ',' => {
                // 数组内 , 后不期望 key，对象内 , 后期望 key
                expect_key = bracket_stack.last() == Some(&'{');
                result.push(',');
            }
            // 单行注释 → 移除
            '/' if i + 1 < len && chars[i + 1] == '/' => {
                i += 2;
                while i < len && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            _ => {
                result.push(c);
            }
        }
        i += 1;
    }

    // 闭合未关闭的字符串
    if in_string {
        result.push('"');
    }

    // 尾部清理
    let mut fixed = result.trim_end().to_string();

    // 移除数组/对象闭合后的多余引号：]" → ], }" → }
    let bytes = fixed.as_bytes();
    let blen = bytes.len();
    if blen >= 2 {
        let last = bytes[blen - 1];
        let prev = bytes[blen - 2];
        if last == b'"' && (prev == b']' || prev == b'}') {
            fixed.pop();
        }
    }

    // 移除尾部逗号
    while let Some(last) = fixed.chars().last() {
        if last == ',' {
            fixed.pop();
            fixed = fixed.trim_end().to_string();
        } else {
            break;
        }
    }

    // 补全剩余未闭合的括号
    while let Some(top) = bracket_stack.pop() {
        fixed.push(if top == '{' { '}' } else { ']' });
    }

    fixed
}

/// 判断 LLM 错误是否由响应截断引起（用于触发 retry）
pub(super) fn is_truncation_error(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        let s = c.to_string();
        s.contains("EOF while parsing")
            || s.contains("unexpected end of input")
            || s.contains("response body is not valid UTF-8")
    })
}

/// 解析 LLM 响应为结构化类型（模型适配 → JSON 提取 → 修复 → serde 解析）
pub(super) fn parse_json_response<D: DeserializeOwned + Send>(response: &str) -> Result<D> {
    let normalized = super::super::model_adaptation::normalize_llm_content(response);
    let raw_json = extract_json_str(normalized.as_ref());
    let json_str = repair_llm_json(&raw_json);

    if json_str.trim().is_empty() {
        tracing::warn!(
            "[JSON parse] content empty after normalize+extract: raw_response_len={}",
            response.len()
        );
        anyhow::bail!("LLM response content is empty after extraction");
    }

    // 直接解析
    if let Ok(parsed) = serde_json::from_str::<D>(&json_str) {
        return Ok(parsed);
    }

    // 解析失败，输出诊断信息
    let parse_err = match serde_json::from_str::<D>(&json_str) {
        Ok(_) => unreachable!(),
        Err(e) => e,
    };
    let error_line = parse_err.line();
    let lines: Vec<&str> = json_str.lines().collect();
    let start = error_line.saturating_sub(4);
    let end = (error_line + 2).min(lines.len());
    let error_snippet: String = lines[start..end]
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let line_num = start + i + 1;
            let marker = if line_num == error_line { ">>>" } else { "   " };
            format!("{} {:4}: {}", marker, line_num, l)
        })
        .collect::<Vec<_>>()
        .join("\n");

    tracing::error!(
        "[JSON parse] {} at line {} col {} (json_len={}):\n{}\n--- Full JSON ---\n{}",
        parse_err,
        error_line,
        parse_err.column(),
        json_str.len(),
        error_snippet,
        json_str
    );
    Err(parse_err.into())
}
