// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::super::FormulaEngine;
    use std::collections::HashMap;

    #[test]
    fn test_basic_arithmetic() {
        let engine = FormulaEngine::new();
        let ctx = HashMap::new();

        assert_eq!(engine.evaluate("1 + 2", &ctx).unwrap(), 3.0);
        assert_eq!(engine.evaluate("10 - 3", &ctx).unwrap(), 7.0);
        assert_eq!(engine.evaluate("4 * 5", &ctx).unwrap(), 20.0);
        assert_eq!(engine.evaluate("20 / 4", &ctx).unwrap(), 5.0);
    }

    #[test]
    fn test_operator_precedence() {
        let engine = FormulaEngine::new();
        let ctx = HashMap::new();

        assert_eq!(engine.evaluate("2 + 3 * 4", &ctx).unwrap(), 14.0);
        assert_eq!(engine.evaluate("(2 + 3) * 4", &ctx).unwrap(), 20.0);
        assert_eq!(engine.evaluate("10 - 2 * 3", &ctx).unwrap(), 4.0);
    }

    #[test]
    fn test_int_context() {
        let engine = FormulaEngine::new();
        let mut ctx = HashMap::new();
        ctx.insert("strength".to_string(), 30);
        ctx.insert("constitution".to_string(), 25);

        assert_eq!(
            engine.evaluate_int("100 + constitution * 2", &ctx).unwrap(),
            150
        );
        assert_eq!(engine.evaluate_int("50 + strength * 2", &ctx).unwrap(), 110);
    }

    #[test]
    fn test_float_context() {
        let engine = FormulaEngine::new();
        let mut ctx = HashMap::new();
        ctx.insert("agility".to_string(), 20.0);

        let result = engine.evaluate("0.05 + agility * 0.005", &ctx).unwrap();
        assert!((result - 0.15).abs() < 0.0001);
    }

    #[test]
    fn test_functions() {
        let engine = FormulaEngine::new();
        let ctx = HashMap::new();

        assert_eq!(engine.evaluate("max(10, 20)", &ctx).unwrap(), 20.0);
        assert_eq!(engine.evaluate("min(10, 20)", &ctx).unwrap(), 10.0);
        assert_eq!(engine.evaluate("floor(3.7)", &ctx).unwrap(), 3.0);
        assert_eq!(engine.evaluate("ceil(3.2)", &ctx).unwrap(), 4.0);
    }

    #[test]
    fn test_int_with_extras() {
        let engine = FormulaEngine::new();
        let mut int_ctx = HashMap::new();
        int_ctx.insert("strength".to_string(), 30);

        let mut float_extras = HashMap::new();
        float_extras.insert("weapon_multiplier".to_string(), 1.5);

        // 30 * 1.5 = 45.0 -> floor 45
        assert_eq!(
            engine
                .evaluate_int_with_extras("strength * weapon_multiplier", &int_ctx, &float_extras)
                .unwrap(),
            45
        );
    }

    #[test]
    fn test_evaluate_max() {
        let engine = FormulaEngine::new();
        let mut ctx = HashMap::new();
        ctx.insert("constitution".to_string(), 20);

        // 公式求值
        assert_eq!(
            engine.evaluate_max(&Some("100 + constitution * 2".to_string()), 50.0, &ctx),
            140.0
        );

        // 纯数字 fallback
        assert_eq!(
            engine.evaluate_max(&Some("255".to_string()), 100.0, &ctx),
            255.0
        );

        // None -> default
        assert_eq!(engine.evaluate_max(&None, 100.0, &ctx), 100.0);
    }

    #[test]
    fn test_validation() {
        let engine = FormulaEngine::new();

        let known_attrs = vec!["strength", "constitution"];
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
        assert!(
            engine
                .validate_formula("100 +", Some(&known_attrs))
                .is_err()
        );
    }

    #[test]
    fn test_negative_numbers() {
        let engine = FormulaEngine::new();
        let ctx = HashMap::new();

        assert_eq!(engine.evaluate("-5", &ctx).unwrap(), -5.0);
        assert_eq!(engine.evaluate("-(10 + 5)", &ctx).unwrap(), -15.0);
    }

    #[test]
    fn test_float_result_floored_to_int() {
        let engine = FormulaEngine::new();
        let mut ctx = HashMap::new();
        ctx.insert("val".to_string(), 7);

        // 7 / 2 = 3.5 -> floor 3
        assert_eq!(engine.evaluate_int("val / 2", &ctx).unwrap(), 3);
    }
}
