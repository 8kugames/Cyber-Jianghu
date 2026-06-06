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
    fn transmission_session_for_whisper() {
        let config = parse(
            r#"
                name: "私语"
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
                name: "休息"
                description: "未配置 display_name"
            "#,
        );
        assert_eq!(without.display_name, None);

        let with = parse(
            r#"
                name: "休息"
                display_name: "静修"
                description: "display_name 用于展示美化"
            "#,
        );
        assert_eq!(with.display_name, Some("静修".to_string()));
    }
}
