// ============================================================================
// persona_event_rules.yaml 加载测试
// ============================================================================
//
// 验证:
// 1. 26 规则正确加载
// 2. 损坏 YAML → fail-fast
// 3. 缺失文件 → fail-fast
// 4. 空 rules → fail-fast
// 5. schema 范围校验
// 6. (过渡期 snapshot) YAML 加载后行为与原 hardcoded 26 条等价
// ============================================================================

use cyber_jianghu_agent::component::persona::event_mapper::EventType;
use cyber_jianghu_agent::component::persona::rules_loader::load_event_trait_rules;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn real_yaml_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("crates/server/config/persona_event_rules.yaml")
}

#[test]
fn test_load_yaml_succeeds_with_26_rules() {
    let mapper = load_event_trait_rules(&real_yaml_path()).expect("YAML 26 规则必须可加载");
    assert_eq!(mapper.rules().len(), 26, "26 规则是创世基线");
}

#[test]
fn test_yaml_load_failure_is_fatal() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("broken.yaml");
    fs::write(
        &path,
        "rules:\n  - event_type: Attacked\n    trait_name: 愤怒\n  # 缺 base_delta / weight\n",
    )
    .unwrap();
    let result = load_event_trait_rules(&path);
    assert!(result.is_err(), "损坏 YAML 必须 fail-fast");
}

#[test]
fn test_yaml_missing_file_is_fatal() {
    let path = PathBuf::from("/tmp/nonexistent_persona_event_rules_xyz.yaml");
    let result = load_event_trait_rules(&path);
    assert!(result.is_err(), "缺失文件必须 fail-fast,无静默 fallback");
}

#[test]
fn test_yaml_empty_rules_is_fatal() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("empty.yaml");
    fs::write(&path, "rules: []\n").unwrap();
    let result = load_event_trait_rules(&path);
    assert!(result.is_err(), "空 rules 必须 fail-fast");
}

#[test]
fn test_yaml_schema_validation() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("invalid.yaml");
    fs::write(
        &path,
        "rules:\n  - event_type: Attacked\n    trait_name: \"\"\n    base_delta: 200\n    weight: 1.0\n",
    )
    .unwrap();
    let result = load_event_trait_rules(&path);
    assert!(result.is_err(), "schema 不匹配必须 fail-fast");
}

#[test]
fn test_yaml_loaded_rules_match_hardcoded_baseline() {
    let mapper = load_event_trait_rules(&real_yaml_path()).expect("YAML 加载成功");

    let expect = [
        (EventType::Attacked, "愤怒", 15_i16, 1.2_f32),
        (EventType::Deceived, "信任", -20, 1.5),
        (EventType::Helped, "信任", 10, 1.2),
        (EventType::BattleWin, "自信", 15, 1.3),
        (EventType::BattleLose, "恐惧", 20, 1.4),
    ];

    for (event_type, trait_name, base_delta, weight) in expect {
        let rule = mapper
            .rules()
            .iter()
            .find(|r| r.event_type == event_type && r.trait_name == trait_name)
            .unwrap_or_else(|| panic!("缺少规则: {:?}/{}", event_type, trait_name));
        assert_eq!(
            rule.base_delta, base_delta,
            "base_delta 不匹配: {:?}/{}",
            event_type, trait_name
        );
        assert_eq!(
            rule.weight, weight,
            "weight 不匹配: {:?}/{}",
            event_type, trait_name
        );
    }
}
