// ============================================================================
// 技能定义类型
// ============================================================================

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 技能定义
///
/// 从 SKILL.md 文件解析而来。frontmatter 为结构化元数据，body 为行为指令文本。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDefinition {
    /// 技能名称（中文）
    pub name: String,
    /// 简短描述
    pub description: String,
    /// 分类（martial/survival/social/economic）
    pub category: String,
    /// 触发场景描述列表
    #[serde(default)]
    pub triggers: Vec<String>,
    /// SKILL.md body 内容（行为指令 markdown）
    #[serde(skip)]
    pub content: String,
}

/// 技能 frontmatter 结构（仅 YAML 部分）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
    pub category: String,
    #[serde(default)]
    pub triggers: Vec<String>,
}

/// 从 SKILL.md 解析出 SkillDefinition
///
/// SKILL.md 格式: YAML frontmatter（`---` 包裹）+ markdown body
impl SkillDefinition {
    /// 从 SKILL.md 文件内容解析
    pub fn from_skill_md(skill_id: &str, content: &str) -> anyhow::Result<Self> {
        let (frontmatter_str, body) = split_frontmatter(content);

        let fm: SkillFrontmatter = serde_yaml::from_str(frontmatter_str)
            .map_err(|e| anyhow::anyhow!("解析技能 {} frontmatter 失败: {}", skill_id, e))?;

        Ok(Self {
            name: fm.name,
            description: fm.description,
            category: fm.category,
            triggers: fm.triggers,
            content: body.trim().to_string(),
        })
    }
}

/// 分离 YAML frontmatter 和 markdown body
///
/// 输入格式:
/// ```md
/// ---
/// name: ...
/// ---
/// # Body content
/// ```
fn split_frontmatter(content: &str) -> (&str, &str) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return ("", content);
    }

    // 跳过第一个 ---
    let after_first = &trimmed[3..];
    let rest = after_first.trim_start_matches(['-', '\r', '\n']);

    // 找第二个 ---
    if let Some(pos) = rest.find("\n---") {
        let (fm, body) = rest.split_at(pos);
        // 跳过第二个 --- 及其换行
        let body = &body[4..]; // skip \n---
        let body = body.trim_start_matches(['-', '\r', '\n']);
        (fm.trim(), body)
    } else {
        (rest.trim(), "")
    }
}

/// 技能数据类型别名
pub type SkillsData = HashMap<String, SkillDefinition>;
