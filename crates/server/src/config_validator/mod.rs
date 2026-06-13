use serde::Deserialize;

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
