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
    let parts: Vec<&str> = pattern.split('.').flat_map(|part| {
        if let Some(stripped) = part.strip_suffix("[*]") {
            if stripped.is_empty() {
                vec!["[*]"]
            } else {
                vec![stripped, "[*]"]
            }
        } else {
            vec![part]
        }
    }).collect();
    walk(value, &parts)
}

fn walk<'a>(value: &'a Value, parts: &[&str]) -> Vec<&'a Value> {
    if parts.is_empty() {
        return vec![value];
    }
    match parts[0] {
        "*" => match value {
            Value::Mapping(map) => {
                map.values().flat_map(|v| walk(v, &parts[1..])).collect()
            }
            _ => vec![],
        },
        "[*]" => match value {
            Value::Sequence(seq) => {
                seq.iter().flat_map(|v| walk(v, &parts[1..])).collect()
            }
            _ => vec![],
        },
        key => match value.get(key) {
            Some(v) => walk(v, &parts[1..]),
            None => vec![],
        },
    }
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
