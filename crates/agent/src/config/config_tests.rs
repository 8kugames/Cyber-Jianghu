//! config 模块单测（自 config.rs 外移，内容未改）

use super::*;

/// 自动注册倒计时默认 30 分钟（1800s）；显式 0 = 禁用；Default 与 serde 缺省一致
#[test]
fn test_auto_register_timeout_default_and_override() {
    let default_cfg: RuntimeConfig = serde_yaml::from_str("{}").unwrap();
    assert_eq!(default_cfg.auto_register_timeout_secs, 1800);
    assert_eq!(RuntimeConfig::default().auto_register_timeout_secs, 1800);

    let disabled: RuntimeConfig = serde_yaml::from_str("auto_register_timeout_secs: 0").unwrap();
    assert_eq!(disabled.auto_register_timeout_secs, 0);
}

/// 测试用 CharacterGenerationConfig (minimal schema)
fn test_cg() -> CharacterGenerationConfig {
    CharacterGenerationConfig {
        world_setting: "武侠架空世界".into(),
        fields: vec![FieldSpec {
            path: "age".into(),
            constraints: FieldConstraints::Integer {
                required: true,
                min: 16,
                max: 60,
            },
        }],
    }
}

#[test]
fn test_character_config_generate_system_prompt() {
    let character = CharacterConfig {
        name: "李逍遥".to_string(),
        age: 25,
        gender: "男".to_string(),
        appearance: Some("身材修长，剑眉星目".to_string()),
        identity: Some("江湖游侠".to_string()),
        personality: vec!["豪爽".to_string(), "重情重义".to_string()],
        values: vec!["侠之大者，为国为民".to_string()],
        language_style: LanguageStyleConfig {
            tone: Some("豪迈".to_string()),
            speech_patterns: vec!["喜欢用江湖切口".to_string()],
        },
        goals: GoalsConfig {
            short_term: Some("寻找失散的师妹".to_string()),
            long_term: Some("成为一代大侠".to_string()),
        },
        ..Default::default()
    };

    let prompt = character.generate_system_prompt();
    assert!(prompt.contains("李逍遥"));
    assert!(prompt.contains("25岁"));
    assert!(prompt.contains("豪爽"));
    assert!(prompt.contains("寻找失散的师妹"));
}

#[test]
fn test_reflector_llm_inheritance() {
    let mut llm = LlmConfig::default();
    llm.provider = "ollama".to_string();
    llm.model = Some("qwen2.5:14b".to_string());

    let config = Config {
        server: ServerConfig::default(),
        runtime: RuntimeConfig::default(),
        llm,
        llm_reflector: None,
        memory: MemoryConfig::default(),
        game_rules: None,
        config_path: PathBuf::from("/test/config.yaml"),
        servers_dir: PathBuf::new(),
        earth_soul: crate::soul::earth::config::EarthSoulConfig::default(),
        token_optimization: TokenOptimizationConfig::default(),
        update: UpdateConfig::default(),
        character_generation: test_cg(),
    };
    assert_eq!(
        config.get_reflector_llm_config().model,
        Some("qwen2.5:14b".to_string())
    );
}

#[test]
fn test_reflector_llm_override() {
    let mut llm = LlmConfig::default();
    llm.provider = "ollama".to_string();
    llm.model = Some("qwen2.5:14b".to_string());

    let mut llm_reflector = LlmConfig::default();
    llm_reflector.provider = "ollama".to_string();
    llm_reflector.model = Some("qwen2.5:32b".to_string());

    let config = Config {
        server: ServerConfig::default(),
        runtime: RuntimeConfig::default(),
        llm,
        llm_reflector: Some(llm_reflector),
        memory: MemoryConfig::default(),
        game_rules: None,
        config_path: PathBuf::from("/test/config.yaml"),
        servers_dir: PathBuf::new(),
        earth_soul: crate::soul::earth::config::EarthSoulConfig::default(),
        token_optimization: TokenOptimizationConfig::default(),
        update: UpdateConfig::default(),
        character_generation: test_cg(),
    };
    assert_eq!(
        config.get_reflector_llm_config().model,
        Some("qwen2.5:32b".to_string())
    );
}
