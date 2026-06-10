#[cfg(test)]
mod tests {
    use cyber_jianghu_server::game_data::types::actions::{ActionConfigEntry, Transmission};

    fn parse(yaml: &str) -> ActionConfigEntry {
        serde_yaml::from_str(yaml).expect("YAML 解析失败")
    }

    #[test]
    fn transmission_omitted_defaults_to_broadcast() {
        let config = parse(
            r#"
                name: "测试"
                description: "fail-fast 默认 broadcast,显式标注才 silent"
            "#,
        );
        assert_eq!(config.transmission, Transmission::Broadcast);
        assert_eq!(config.display_name, None);
    }

    #[test]
    fn transmission_explicit_silent() {
        let config = parse(
            r#"
                name: "测试"
                description: "..."
                transmission: silent
            "#,
        );
        assert_eq!(config.transmission, Transmission::Silent);
    }

    #[test]
    fn transmission_broadcast_for_speak() {
        let config = parse(
            r#"
                name: "说话"
                description: "..."
                transmission: broadcast
            "#,
        );
        assert_eq!(config.transmission, Transmission::Broadcast);
    }

    #[test]
    fn transmission_session_for_speak() {
        let config = parse(
            r#"
                name: "说话"
                description: "..."
                transmission: session
            "#,
        );
        assert_eq!(config.transmission, Transmission::Session);
    }

    #[test]
    fn display_name_optional_and_overrides_when_set() {
        let without = parse(
            r#"
                name: "休整"
                description: "未配置 display_name"
            "#,
        );
        assert_eq!(without.display_name, None);

        let with = parse(
            r#"
                name: "休整"
                display_name: "静修"
                description: "display_name 用于展示美化"
            "#,
        );
        assert_eq!(with.display_name, Some("静修".to_string()));
    }

    #[test]
    fn validator_kind_optional_with_snake_case_values() {
        let without = parse(
            r#"
                name: "休整"
                description: "无预验证"
            "#,
        );
        assert_eq!(without.validator_kind, None);

        let recipe = parse(
            r#"
                name: "制造"
                description: "..."
                validator_kind: recipe_knowledge
            "#,
        );
        assert!(matches!(
            recipe.validator_kind,
            Some(cyber_jianghu_server::game_data::types::actions::ValidatorKind::RecipeKnowledge)
        ));

        let teach = parse(
            r#"
                name: "传授"
                description: "..."
                validator_kind: teach_recipe
            "#,
        );
        assert!(matches!(
            teach.validator_kind,
            Some(cyber_jianghu_server::game_data::types::actions::ValidatorKind::TeachRecipe)
        ));
    }

    #[test]
    fn highlight_kind_optional_with_snake_case_values() {
        let without = parse(
            r#"
                name: "休整"
                description: "无 highlight"
            "#,
        );
        assert_eq!(without.highlight_kind, None);

        let dialogue = parse(
            r#"
                name: "说话"
                description: "..."
                highlight_kind: dialogue
            "#,
        );
        assert!(matches!(
            dialogue.highlight_kind,
            Some(cyber_jianghu_server::game_data::types::actions::HighlightKind::Dialogue)
        ));

        let combat = parse(
            r#"
                name: "攻击"
                description: "..."
                highlight_kind: combat
            "#,
        );
        assert!(matches!(
            combat.highlight_kind,
            Some(cyber_jianghu_server::game_data::types::actions::HighlightKind::Combat)
        ));

        let social = parse(
            r#"
                name: "予"
                description: "..."
                highlight_kind: social
            "#,
        );
        assert!(matches!(
            social.highlight_kind,
            Some(cyber_jianghu_server::game_data::types::actions::HighlightKind::Social)
        ));
    }
}
