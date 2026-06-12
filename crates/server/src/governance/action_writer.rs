use anyhow::{Context, Result};
use std::path::Path;
use tracing::info;

/// 将新 action 条目追加到 actions.yaml
///
/// 如果 action_name 已存在则跳过（幂等）。
pub fn append_action_to_yaml(
    config_dir: &Path,
    action_name: &str,
    entry: &serde_yaml::Value,
) -> Result<()> {
    let yaml_path = config_dir.join("actions.yaml");
    let content = std::fs::read_to_string(&yaml_path).context("读取 actions.yaml 失败")?;

    let mut doc: serde_yaml::Value =
        serde_yaml::from_str(&content).context("解析 actions.yaml 失败")?;

    let data = doc.get_mut("data").context("actions.yaml 缺少 data 字段")?;

    // 幂等：已存在则跳过
    if data.get(action_name).is_some() {
        info!(
            action_name = %action_name,
            "action_writer: 动作已存在，跳过"
        );
        return Ok(());
    }

    data.as_mapping_mut().context("data 不是 mapping")?.insert(
        serde_yaml::Value::String(action_name.to_string()),
        entry.clone(),
    );

    // 更新 meta.updated_at
    if let Some(meta) = doc.get_mut("meta")
        && let Some(meta_map) = meta.as_mapping_mut()
    {
        meta_map.insert(
            serde_yaml::Value::String("updated_at".to_string()),
            serde_yaml::Value::String(chrono::Utc::now().format("%Y-%m-%d").to_string()),
        );
    }

    let new_content = serde_yaml::to_string(&doc).context("序列化 actions.yaml 失败")?;
    std::fs::write(&yaml_path, new_content).context("写入 actions.yaml 失败")?;

    info!(
        action_name = %action_name,
        "action_writer: 新动作已写入 actions.yaml"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_append_action_creates_entry() {
        let dir = tempfile::tempdir().unwrap();
        let yaml_content = r#"
version: "2.0"
meta:
  created_at: "2026-01-01"
  updated_at: "2026-01-01"
data:
  攻击:
    name: "攻击"
    description: "attack"
    category: combat
    ooc_risk: medium
    transmission: silent
    validation: {}
    requirements: []
"#;
        fs::write(dir.path().join("actions.yaml"), yaml_content).unwrap();

        let entry = serde_yaml::to_value(serde_json::json!({
            "name": "新动作",
            "description": "a new action",
            "category": "social",
            "ooc_risk": "low",
            "transmission": "silent",
            "validation": { "required_fields": [] },
            "requirements": [],
        }))
        .unwrap();

        append_action_to_yaml(dir.path(), "新动作", &entry).unwrap();

        let result = fs::read_to_string(dir.path().join("actions.yaml")).unwrap();
        assert!(result.contains("新动作"));
        assert!(result.contains("a new action"));
    }

    #[test]
    fn test_append_action_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let yaml_content = r#"
version: "2.0"
meta:
  created_at: "2026-01-01"
  updated_at: "2026-01-01"
data:
  攻击:
    name: "攻击"
    description: "attack"
    category: combat
    ooc_risk: medium
    transmission: silent
    validation: {}
    requirements: []
"#;
        fs::write(dir.path().join("actions.yaml"), yaml_content).unwrap();

        let entry = serde_yaml::to_value(serde_json::json!({
            "name": "攻击",
            "description": "should not overwrite",
        }))
        .unwrap();

        // Should not fail, should skip
        append_action_to_yaml(dir.path(), "攻击", &entry).unwrap();

        let result = fs::read_to_string(dir.path().join("actions.yaml")).unwrap();
        assert!(result.contains("attack"));
        assert!(!result.contains("should not overwrite"));
    }
}
