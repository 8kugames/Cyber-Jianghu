// ============================================================================
// 技能加载器 — 递归扫描 skills/ 目录中的 SKILL.md 文件
// ============================================================================

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;

use crate::game_data::types::skills::{SkillDefinition, SkillsData};

/// 递归扫描 skills/ 目录，加载所有 SKILL.md 文件
///
/// 目录结构: `skills/{category}/{skill_id}/SKILL.md`
/// skill_id = 相对于 skills/ 目录的路径（如 `martial/sword-basic`）
pub fn load_skills<P: AsRef<Path>>(skills_dir: P) -> Result<SkillsData> {
    let skills_dir = skills_dir.as_ref();
    let mut skills = HashMap::new();

    if !skills_dir.exists() {
        tracing::warn!("技能目录不存在: {:?}", skills_dir);
        return Ok(skills);
    }

    scan_skill_dir(skills_dir, skills_dir, &mut skills)?;
    tracing::info!("加载了 {} 个技能定义", skills.len());
    Ok(skills)
}

/// 递归扫描目录，查找 SKILL.md 文件
fn scan_skill_dir(
    dir: &Path,
    root: &Path,
    skills: &mut SkillsData,
) -> Result<()> {
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("读取技能目录失败: {:?}", dir))?;

    for entry in entries {
        let entry = entry.context("读取目录条目失败")?;
        let path = entry.path();

        if path.is_dir() {
            // 检查是否包含 SKILL.md
            let skill_file = path.join("SKILL.md");
            if skill_file.exists() {
                let skill_id = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");

                let content = std::fs::read_to_string(&skill_file)
                    .with_context(|| format!("读取技能文件失败: {:?}", skill_file))?;

                let def = SkillDefinition::from_skill_md(&skill_id, &content)
                    .with_context(|| format!("解析技能 {:?} 失败", skill_id))?;

                tracing::debug!("加载技能: {} -> {}", skill_id, def.name);
                skills.insert(skill_id, def);
            } else {
                // 递归子目录
                scan_skill_dir(&path, root, skills)?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_skills_nonexistent_dir() {
        let result = load_skills("/nonexistent/skills/dir");
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_parse_skill_md_frontmatter() {
        let content = r#"---
name: "测试技能"
description: "测试描述"
category: martial
triggers:
  - "触发场景"
---

# 测试技能

## 行为准则
- 测试行为
"#;
        let def = SkillDefinition::from_skill_md("test-skill", content).unwrap();
        assert_eq!(def.name, "测试技能");
        assert_eq!(def.category, "martial");
        assert_eq!(def.triggers, vec!["触发场景"]);
        assert!(def.content.contains("行为准则"));
    }
}
