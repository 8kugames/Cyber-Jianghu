// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use super::super::{FormulaEngine, context::PrimaryAttributeProvider};
    use std::collections::HashMap;

    struct TestProvider {
        attributes: HashMap<String, u8>,
    }

    impl TestProvider {
        fn new() -> Self {
            let mut attributes = HashMap::new();
            attributes.insert("strength".to_string(), 10);
            attributes.insert("agility".to_string(), 10);
            attributes.insert("constitution".to_string(), 10);
            attributes.insert("intelligence".to_string(), 10);
            attributes.insert("charisma".to_string(), 10);
            attributes.insert("luck".to_string(), 10);
            Self { attributes }
        }

        fn with_values(values: Vec<(&str, u8)>) -> Self {
            let mut attributes = HashMap::new();
            for (name, value) in values {
                attributes.insert(name.to_string(), value);
            }
            Self { attributes }
        }
    }

    impl PrimaryAttributeProvider for TestProvider {
        fn get_attribute(&self, name: &str) -> Option<u8> {
            self.attributes.get(name).copied()
        }
    }

    #[test]
    fn test_basic_arithmetic() {
        let engine = FormulaEngine::new();
        let provider = TestProvider::new();

        assert_eq!(engine.evaluate("1 + 2", &provider).unwrap(), 3.0);
        assert_eq!(engine.evaluate("10 - 3", &provider).unwrap(), 7.0);
        assert_eq!(engine.evaluate("4 * 5", &provider).unwrap(), 20.0);
        assert_eq!(engine.evaluate("20 / 4", &provider).unwrap(), 5.0);
    }

    #[test]
    fn test_operator_precedence() {
        let engine = FormulaEngine::new();
        let provider = TestProvider::new();

        assert_eq!(engine.evaluate("2 + 3 * 4", &provider).unwrap(), 14.0);
        assert_eq!(engine.evaluate("(2 + 3) * 4", &provider).unwrap(), 20.0);
        assert_eq!(engine.evaluate("10 - 2 * 3", &provider).unwrap(), 4.0);
    }

    #[test]
    fn test_variable_replacement() {
        let engine = FormulaEngine::new();
        let provider = TestProvider::with_values(vec![
            ("strength", 30),
            ("agility", 20),
            ("constitution", 25),
            ("intelligence", 15),
            ("charisma", 10),
            ("luck", 40),
        ]);

        assert_eq!(
            engine
                .evaluate("100 + constitution * 2", &provider)
                .unwrap(),
            150.0
        );
        assert_eq!(
            engine.evaluate("50 + strength * 2", &provider).unwrap(),
            110.0
        );
        assert!(
            (engine
                .evaluate("0.05 + agility * 0.005", &provider)
                .unwrap()
                - 0.15)
                .abs()
                < 0.0001
        );
    }

    #[test]
    fn test_functions() {
        let engine = FormulaEngine::new();
        let provider = TestProvider::new();

        assert_eq!(engine.evaluate("max(10, 20)", &provider).unwrap(), 20.0);
        assert_eq!(engine.evaluate("min(10, 20)", &provider).unwrap(), 10.0);
        assert_eq!(engine.evaluate("floor(3.7)", &provider).unwrap(), 3.0);
        assert_eq!(engine.evaluate("ceil(3.2)", &provider).unwrap(), 4.0);
    }

    #[test]
    fn test_complex_formulas() {
        let engine = FormulaEngine::new();
        let provider = TestProvider::with_values(vec![
            ("strength", 30),
            ("agility", 25),
            ("constitution", 20),
            ("intelligence", 15),
            ("charisma", 10),
            ("luck", 40),
        ]);

        // HP 公式: 100 + constitution * 2
        assert_eq!(
            engine
                .evaluate("100 + constitution * 2", &provider)
                .unwrap(),
            140.0
        );

        // 闪避率公式: 0.05 + agility * 0.005
        assert_eq!(
            engine
                .evaluate("0.05 + agility * 0.005", &provider)
                .unwrap(),
            0.175
        );

        // 复杂公式: (100 + constitution * 2) * (1 + strength * 0.01)
        assert_eq!(
            engine
                .evaluate(
                    "(100 + constitution * 2) * (1 + strength * 0.01)",
                    &provider
                )
                .unwrap(),
            182.0
        );
    }

    #[test]
    fn test_negative_numbers() {
        let engine = FormulaEngine::new();
        let provider = TestProvider::new();

        assert_eq!(engine.evaluate("-5", &provider).unwrap(), -5.0);
        assert_eq!(engine.evaluate("10 + -3", &provider).unwrap(), 7.0);
        assert_eq!(engine.evaluate("-(10 + 5)", &provider).unwrap(), -15.0);
    }

    #[test]
    fn test_validation() {
        let engine = FormulaEngine::new();

        // 已知属性列表
        let known_attrs = vec![
            "strength",
            "agility",
            "constitution",
            "intelligence",
            "charisma",
            "luck",
        ];

        // 有效公式（使用已知属性）
        assert!(
            engine
                .validate_formula("100 + constitution * 2", Some(&known_attrs))
                .is_ok()
        );
        assert!(
            engine
                .validate_formula("max(10, 20)", Some(&known_attrs))
                .is_ok()
        );

        // 无效公式（语法错误）
        assert!(
            engine
                .validate_formula("100 +", Some(&known_attrs))
                .is_err()
        );

        // 未知属性（当提供属性列表时会失败）
        assert!(
            engine
                .validate_formula("unknown_var * 2", Some(&known_attrs))
                .is_err()
        );

        // 不提供属性列表时，所有变量都被视为有效
        assert!(engine.validate_formula("unknown_var * 2", None).is_ok());
    }

    #[test]
    fn test_division_by_zero() {
        let engine = FormulaEngine::new();
        let provider = TestProvider::new();

        assert!(engine.evaluate("10 / 0", &provider).is_err());
    }
}
