use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_yaml::Value;

#[derive(Debug, Clone, Deserialize)]
pub struct ValidationRule {
    pub source_type: String,
    pub source_field: String,
    pub target_type: String,
    pub target_key: String,
}

#[derive(Debug, Clone, Deserialize)]
struct RuleList {
    rules: Vec<ValidationRule>,
}

#[derive(Debug, Default)]
pub struct ValidationResult {
    pub passed: usize,
    pub failed: usize,
    pub violations: Vec<Violation>,
}

#[derive(Debug)]
pub struct Violation {
    pub rule_index: usize,
    pub source_type: String,
    pub source_value: String,
    pub target_type: String,
    pub message: String,
}

pub fn load_rules() -> Result<Vec<ValidationRule>, String> {
    let config_dir = crate::paths::get_config_dir();
    let rules_path = config_dir.join("validation_rules.yaml");

    let content = std::fs::read_to_string(&rules_path)
        .map_err(|e| format!("Failed to read {}: {}", rules_path.display(), e))?;

    let rule_list: RuleList = serde_yaml::from_str(&content)
        .map_err(|e| format!("Failed to parse validation_rules.yaml: {}", e))?;

    if rule_list.rules.is_empty() {
        return Err("Validation rules list is empty".to_string());
    }

    Ok(rule_list.rules)
}

pub fn resolve_pattern<'a>(value: &'a Value, pattern: &str) -> Vec<&'a Value> {
    let parts: Vec<&str> = pattern
        .split('.')
        .flat_map(|part| {
            if let Some(stripped) = part.strip_suffix("[*]") {
                if stripped.is_empty() {
                    vec!["[*]"]
                } else {
                    vec![stripped, "[*]"]
                }
            } else {
                vec![part]
            }
        })
        .collect();
    walk(value, &parts)
}

fn walk<'a>(value: &'a Value, parts: &[&str]) -> Vec<&'a Value> {
    if parts.is_empty() {
        return vec![value];
    }
    match parts[0] {
        "*" => match value {
            Value::Mapping(map) => map.values().flat_map(|v| walk(v, &parts[1..])).collect(),
            _ => vec![],
        },
        "[*]" => match value {
            Value::Sequence(seq) => seq.iter().flat_map(|v| walk(v, &parts[1..])).collect(),
            _ => vec![],
        },
        key => match value.get(key) {
            Some(v) => walk(v, &parts[1..]),
            None => vec![],
        },
    }
}

fn load_yaml(path: &Path) -> Result<Value, String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("读取 {:?} 失败: {}", path, e))?;
    serde_yaml::from_str(&content).map_err(|e| format!("解析 {:?} 失败: {}", path, e))
}

fn resolve_target_set(
    config_dir: &Path,
    target_type: &str,
    target_key: &str,
) -> Result<HashSet<String>, String> {
    let value = match target_type {
        "actions" => load_yaml(&config_dir.join("actions.yaml"))?,
        "attributes" => load_yaml(&config_dir.join("attributes.yaml"))?,
        "items" => load_yaml(&config_dir.join("items.yaml"))?,
        "recipes" => load_yaml(&config_dir.join("recipes.yaml"))?,
        "locations" => load_yaml(&config_dir.join("locations.yaml"))?,
        "action_evolution" => load_yaml(&config_dir.join("action_evolution.yaml"))?,
        "skills" => return list_skill_categories(config_dir),
        "skill_md" => return Ok(HashSet::new()),
        _ => return Err(format!("未知 target_type: {}", target_type)),
    };
    let matched = resolve_pattern(&value, target_key);
    // 支持两种模式：
    // 1. 值直接是字符串 (items.data.[*].item_id → "馒头")
    // 2. 值是 Mapping，其 keys 是目标值 (attributes.data.*.attributes.* → {hp: {...}, stamina: {...}})
    let set: HashSet<String> = matched
        .iter()
        .flat_map(|v| {
            if let Some(s) = v.as_str() {
                vec![s.to_string()]
            } else if let Value::Mapping(map) = v {
                map.keys()
                    .filter_map(|k| k.as_str().map(|s| s.to_string()))
                    .collect()
            } else {
                vec![]
            }
        })
        .collect();
    Ok(set)
}

fn list_skill_categories(config_dir: &Path) -> Result<HashSet<String>, String> {
    let skills_dir = config_dir.join("skills");
    let mut categories = HashSet::new();
    if skills_dir.exists() {
        for entry in
            std::fs::read_dir(&skills_dir).map_err(|e| format!("读取 skills/ 目录失败: {}", e))?
        {
            let entry = entry.map_err(|e| format!("读取 entry 失败: {}", e))?;
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
                && let Some(name) = entry.file_name().to_str()
            {
                categories.insert(name.to_string());
            }
        }
    }
    Ok(categories)
}

fn find_skill_md_files(config_dir: &Path) -> Vec<PathBuf> {
    let skills_dir = config_dir.join("skills");
    if !skills_dir.exists() {
        return vec![];
    }
    let mut files = vec![];
    collect_skill_md_files(&skills_dir, &mut files);
    files
}

fn collect_skill_md_files(dir: &Path, files: &mut Vec<PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_skill_md_files(&path, files);
            } else if path.file_name().and_then(|n| n.to_str()) == Some("SKILL.md") {
                files.push(path);
            }
        }
    }
}

fn parse_skill_frontmatter(content: &str) -> Result<Value, String> {
    let content = content.trim();
    if !content.starts_with("---") {
        return Err("SKILL.md 缺少 frontmatter".to_string());
    }
    let after_first = content.trim_start_matches("---").trim();
    let end = after_first
        .find("---")
        .ok_or_else(|| "SKILL.md frontmatter 未正确闭合".to_string())?;
    let yaml_str = &after_first[..end];
    serde_yaml::from_str(yaml_str).map_err(|e| format!("解析 SKILL.md frontmatter 失败: {}", e))
}

fn validate_single_rule(
    rule: &ValidationRule,
    rule_index: usize,
    config_dir: &Path,
    violations: &mut Vec<Violation>,
) -> Result<(), String> {
    match rule.source_type.as_str() {
        "skill_md" => {
            let files = find_skill_md_files(config_dir);
            for file in &files {
                let content = std::fs::read_to_string(file)
                    .map_err(|e| format!("读取 {:?} 失败: {}", file, e))?;
                let frontmatter = parse_skill_frontmatter(&content)?;
                let matched = resolve_pattern(&frontmatter, &rule.source_field);
                for val in matched {
                    if let Some(category) = val.as_str() {
                        // 结构: skills/{category}/{skill_name}/SKILL.md
                        // 验证: category == {category}(grandparent), 即大类目录名
                        let grandparent_dir = file
                            .parent()
                            .and_then(|p| p.parent())
                            .and_then(|p| p.file_name())
                            .and_then(|n| n.to_str())
                            .unwrap_or("");
                        if category != grandparent_dir {
                            violations.push(Violation {
                                rule_index,
                                source_type: "skill_md".to_string(),
                                source_value: format!("{}: category={}", file.display(), category),
                                target_type: "skill_md".to_string(),
                                message: format!(
                                    "SKILL.md category '{}' 与所属大类目录 '{}' 不匹配",
                                    category, grandparent_dir
                                ),
                            });
                        }
                    }
                }
            }
            Ok(())
        }
        _ => {
            let source_path = match rule.source_type.as_str() {
                "actions" => config_dir.join("actions.yaml"),
                "attributes" => config_dir.join("attributes.yaml"),
                "items" => config_dir.join("items.yaml"),
                "recipes" => config_dir.join("recipes.yaml"),
                "locations" => config_dir.join("locations.yaml"),
                "action_evolution" => config_dir.join("action_evolution.yaml"),
                _ => return Err(format!("未知 source_type: {}", rule.source_type)),
            };
            let source_value = load_yaml(&source_path)?;
            let references = resolve_pattern(&source_value, &rule.source_field);
            let targets = resolve_target_set(config_dir, &rule.target_type, &rule.target_key)?;
            for ref_val in &references {
                if let Some(ref_str) = ref_val.as_str()
                    && !targets.contains(ref_str)
                {
                    violations.push(Violation {
                        rule_index,
                        source_type: rule.source_type.clone(),
                        source_value: ref_str.to_string(),
                        target_type: rule.target_type.clone(),
                        message: format!("'{}' 在 {} 中未找到定义", ref_str, rule.target_type),
                    });
                }
            }
            Ok(())
        }
    }
}

pub fn run_all_validations(rules: &[ValidationRule]) -> ValidationResult {
    let mut result = ValidationResult::default();
    let config_dir = crate::paths::get_config_dir();
    for (i, rule) in rules.iter().enumerate() {
        match validate_single_rule(rule, i, &config_dir, &mut result.violations) {
            Ok(()) => result.passed += 1,
            Err(e) => {
                result.failed += 1;
                result.violations.push(Violation {
                    rule_index: i,
                    source_type: rule.source_type.clone(),
                    source_value: String::new(),
                    target_type: rule.target_type.clone(),
                    message: format!("验证执行错误: {}", e),
                });
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml::Value;

    #[test]
    fn test_resolve_simple_key() {
        let value: Value = serde_yaml::from_str("{a: 1, b: 2}").unwrap();
        let result = resolve_pattern(&value, "a");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_i64(), Some(1));
    }

    #[test]
    fn test_resolve_wildcard_map() {
        let value: Value = serde_yaml::from_str("{a: {x: 1}, b: {x: 2}}").unwrap();
        let result = resolve_pattern(&value, "*.x");
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_resolve_array_wildcard() {
        let value: Value = serde_yaml::from_str("{data: [{id: 1}, {id: 2}]}").unwrap();
        let result = resolve_pattern(&value, "data.[*].id");
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].as_i64(), Some(1));
        assert_eq!(result[1].as_i64(), Some(2));
    }

    #[test]
    fn test_resolve_nested_mixed() {
        let yaml = r#"
data:
  攻击:
    requirements:
      - attribute: stamina
      - attribute: strength
  防御:
    requirements:
      - attribute: agility
"#;
        let value: Value = serde_yaml::from_str(yaml).unwrap();
        let result = resolve_pattern(&value, "data.*.requirements[*].attribute");
        let mut extracted: Vec<&str> = result.iter().filter_map(|v| v.as_str()).collect();
        extracted.sort();
        assert_eq!(extracted, vec!["agility", "stamina", "strength"]);
    }

    #[test]
    fn test_resolve_no_match_returns_empty() {
        let value: Value = serde_yaml::from_str("{a: 1}").unwrap();
        let result = resolve_pattern(&value, "b.c");
        assert!(result.is_empty());
    }

    #[test]
    fn test_resolve_wildcard_on_non_map_returns_empty() {
        let value: Value = serde_yaml::from_str("42").unwrap();
        let result = resolve_pattern(&value, "*.x");
        assert!(result.is_empty());
    }
}
