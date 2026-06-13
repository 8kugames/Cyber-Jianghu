//! crates/server/tests/config_integrity_test.rs
//!
//! 配置引用完整性集成测试
//!
//! 加载 validation_rules.yaml 中的规则，对当前所有 YAML 配置文件执行
//! 跨文件引用完整性检查。CI 中作为独立测试 binary 运行。
//!
//! 运行: cargo test --test config_integrity_test
//! 预期: 0 violations（所有引用完整）

#[cfg(test)]
mod tests {
    use cyber_jianghu_server::config_validator::{load_rules, run_all_validations};

    #[test]
    fn test_config_integrity_all_rules() {
        let rules = load_rules().expect("validation_rules.yaml 应能正常加载");
        assert!(!rules.is_empty(), "至少应有一条验证规则");
        assert!(
            rules.len() >= 7,
            "至少应有 7 条规则 (当前: {})",
            rules.len()
        );

        let result = run_all_validations(&rules);

        for v in &result.violations {
            eprintln!(
                "违规 [规则 {}] {} → {}: {}",
                v.rule_index, v.source_type, v.target_type, v.message
            );
            if !v.source_value.is_empty() {
                eprintln!("  引用值: {}", v.source_value);
            }
        }

        assert!(
            result.violations.is_empty(),
            "配置引用完整性检查失败: {} 条违规 (passed={}, failed={})",
            result.violations.len(),
            result.passed,
            result.failed,
        );
    }

    #[test]
    fn test_load_rules_success() {
        let rules = load_rules().expect("加载 rules");
        assert!(!rules.is_empty());
        for (i, rule) in rules.iter().enumerate() {
            assert!(!rule.source_type.is_empty(), "规则 {}: source_type 为空", i);
            assert!(
                !rule.source_field.is_empty(),
                "规则 {}: source_field 为空",
                i
            );
            assert!(!rule.target_type.is_empty(), "规则 {}: target_type 为空", i);
            assert!(!rule.target_key.is_empty(), "规则 {}: target_key 为空", i);
        }
    }

    #[test]
    fn test_source_types_covered() {
        let rules = load_rules().expect("加载 rules");
        let source_types: std::collections::HashSet<&str> =
            rules.iter().map(|r| r.source_type.as_str()).collect();
        assert!(
            source_types.len() >= 3,
            "source_type 覆盖不足: {:?}",
            source_types
        );
    }
}
