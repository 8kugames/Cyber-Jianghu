// ============================================================================
// skill_view 工具定义与执行
// ============================================================================

use crate::component::llm::tool_types::ToolDefinition;

/// skill_view tool 定义
pub fn skill_view_definition() -> ToolDefinition {
    ToolDefinition::new(
        "skill_view",
        "查看已掌握技能的详细行为指引。当你的场景匹配某个已掌握技能时调用此工具。注意：skill_id 必须从已掌握技能列表中选择。如果你不知道 skill_id 说明你不掌握你期望使用的技能",
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "skill_id": {
                    "type": "string",
                    "description": "技能ID（支持简写），如 trust-reading、conflict-navigation、risk-assessment"
                }
            },
            "required": ["skill_id"]
        })),
    )
}

/// 执行 skill_view：从缓存加载 SKILL.md body
pub fn execute_skill_view(
    skill_id: &str,
    skill_cache: &std::collections::HashMap<String, String>,
) -> serde_json::Value {
    // 1. 精确匹配缓存
    if let Some(body) = skill_cache.get(skill_id) {
        return serde_json::json!({
            "skill_id": skill_id,
            "content": body
        });
    }

    // 2. 尾部模糊匹配：LLM 可能传 "trust-reading"，但 cache key 是 "social/trust-reading"
    let fuzzy_key = skill_cache
        .keys()
        .find(|k| k.ends_with(&format!("/{skill_id}")) || k.as_str() == skill_id);
    if let Some(key) = fuzzy_key
        && let Some(body) = skill_cache.get(key)
    {
        return serde_json::json!({
            "skill_id": key,
            "content": body
        });
    }

    // 3. 未找到，返回可用技能列表帮助 LLM 纠正
    let available: Vec<&str> = skill_cache.keys().map(|k| k.as_str()).collect();
    serde_json::json!({
        "error": format!("技能 {} 未找到", skill_id),
        "available_skills": available
    })
}

/// 从 SKILL.md 内容中提取 frontmatter 之后的 body（保留用于测试）
#[allow(dead_code)]
///
/// 注意: 此逻辑必须与 server 端
/// `crates/server/src/game_data/types/skills.rs::split_frontmatter()` 保持同步。
pub(crate) fn extract_skill_body(content: &str) -> String {
    if let Some(pos) = content.find("\n---\n") {
        let after = &content[pos + 5..];
        after.trim().to_string()
    } else if let Some(rest) = content.strip_prefix("---") {
        if let Some(pos) = rest.find("\n---") {
            let after = &rest[pos + 4..];
            after
                .trim_start_matches(['-', '\r', '\n'])
                .trim()
                .to_string()
        } else {
            content.trim().to_string()
        }
    } else {
        content.trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_view_definition_format() {
        let def = skill_view_definition();
        assert_eq!(def.function.name, "skill_view");
        assert!(def.function.parameters.is_some());
    }

    #[test]
    fn test_execute_skill_view_from_cache() {
        let mut cache = std::collections::HashMap::new();
        cache.insert("bargaining".to_string(), "讨价还价行为指引...".to_string());

        let result = execute_skill_view("bargaining", &cache);
        assert_eq!(result["skill_id"], "bargaining");
        assert!(result["content"].is_string());
    }

    #[test]
    fn test_execute_skill_view_not_found() {
        let cache = std::collections::HashMap::new();
        let result = execute_skill_view("nonexistent", &cache);
        assert!(result["error"].is_string());
    }

    #[test]
    fn test_extract_skill_body() {
        let content = "---\nname: 测试\n---\n# 正文\n行为指引";
        assert_eq!(extract_skill_body(content), "# 正文\n行为指引");
    }

    #[test]
    fn test_extract_skill_body_no_frontmatter() {
        let content = "# 直接正文";
        assert_eq!(extract_skill_body(content), "# 直接正文");
    }

    #[test]
    fn test_extract_skill_body_double_separator() {
        // 连续 --- 分隔符：第二个 --- 被识别为闭合分隔符
        let content = "---\n---\nbody";
        assert_eq!(extract_skill_body(content), "body");
    }

    #[test]
    fn test_extract_skill_body_standard_frontmatter() {
        let content = "---\nfoo: bar\n---\n# 正文";
        assert_eq!(extract_skill_body(content), "# 正文");
    }

    #[test]
    fn test_extract_skill_body_unclosed_frontmatter() {
        // 只有开头 --- 无闭合 → 返回原文
        let content = "---\nfoo\nbar";
        assert_eq!(extract_skill_body(content), "---\nfoo\nbar");
    }

    #[test]
    fn test_extract_skill_body_newline_before_separator() {
        // 先匹配 \n---\n 再匹配 strip_prefix("---")
        let content = "---\nname: test\n---\nbody text";
        assert_eq!(extract_skill_body(content), "body text");
    }
}
