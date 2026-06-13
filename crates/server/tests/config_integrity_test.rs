use std::path::Path;

fn resolve_config_dir() -> std::path::PathBuf {
    let cwd = std::env::current_dir().expect("can't get cwd");
    let candidates = [
        cwd.join("crates/server/config"),
        cwd.join("../crates/server/config"),
        cwd.join("config"),
        Path::new("crates/server/config").to_path_buf(),
    ];
    for c in &candidates {
        if c.join("validation_rules.yaml").exists() {
            return c.clone();
        }
    }
    panic!(
        "config/validation_rules.yaml not found (cwd={:?}, tried={:?})",
        cwd, candidates
    );
}

#[test]
fn test_config_integrity_all_rules() {
    let config_dir = resolve_config_dir();
    let rules_path = config_dir.join("validation_rules.yaml");

    let report = cyber_jianghu_server::config_validator::run_validation(&config_dir, &rules_path)
        .expect("config integrity validation failed");

    report.print_summary();

    assert!(
        report.is_all_ok(),
        "config integrity check failed: {} rules failed",
        report.results.iter().filter(|r| !r.is_ok()).count()
    );
}

#[test]
fn test_validation_rules_count() {
    use std::io::Read;

    let config_dir = resolve_config_dir();
    let path = config_dir.join("validation_rules.yaml");

    let mut content = String::new();
    std::fs::File::open(&path)
        .unwrap()
        .read_to_string(&mut content)
        .unwrap();

    let rule_count = content
        .lines()
        .filter(|l| l.trim().starts_with("- source_type:"))
        .count();
    assert!(rule_count >= 7, "Expected >=7 rules, found {}", rule_count);
}

/// Verify all capability groups referenced in action_evolution.yaml
/// have corresponding subdirectories under skills/
#[test]
fn test_skill_dirs_exist_for_capability_groups() {
    let config_dir = resolve_config_dir();
    let path = config_dir.join("action_evolution.yaml");
    let value = cyber_jianghu_server::config_validator::load_config_value(&path)
        .expect("load action_evolution.yaml");

    let groups = cyber_jianghu_server::config_validator::resolve_pattern(
        &value,
        "data.capability_policy.allowed_capability_groups[*]",
    );

    let skills_dir = config_dir.join("skills");
    assert!(skills_dir.exists(), "skills/ directory not found");

    for g in &groups {
        let group_name = g.as_str().expect("group must be a string");
        let dir = skills_dir.join(group_name);
        assert!(
            dir.is_dir(),
            "skills/{}/ directory does not exist (referenced by action_evolution.yaml)",
            group_name
        );
    }
    assert!(!groups.is_empty(), "no capability groups found");
}

/// Ensure $keys-pattern rules have unique refs properly counted
/// (total_refs in report is deduped — functional check)
#[test]
fn test_deduped_refs_in_actions() {
    let config_dir = resolve_config_dir();
    let rules_path = config_dir.join("validation_rules.yaml");
    let report = cyber_jianghu_server::config_validator::run_validation(&config_dir, &rules_path)
        .expect("validation failed");

    let r1 = report
        .results
        .iter()
        .find(|r| r.rule.source_type == "actions")
        .expect("R1 not found");
    assert!(
        r1.total_refs >= 12,
        "R1 should have >=12 raw refs (12 actions all reference stamina), got {}",
        r1.total_refs
    );
    assert!(r1.is_ok());
}

#[test]
fn test_broken_reference_detected() {
    let config_dir = resolve_config_dir();
    let bad_rules_yaml = r#"
rules:
  - source_type: nonexistent_file_xyz
    source_field: "data[*].value"
    target_type: items
    target_key: "data[*].item_id"
"#;
    let bad_rules_path = config_dir.join("_test_bad_rules.yaml");
    std::fs::write(&bad_rules_path, bad_rules_yaml).unwrap();

    let result =
        cyber_jianghu_server::config_validator::run_validation(&config_dir, &bad_rules_path);
    let _ = std::fs::remove_file(&bad_rules_path);

    assert!(result.is_err(), "Expected error for nonexistent config");
}
