use anyhow::{Context, Result};
use std::path::Path;

pub fn load_rules_json(config_dir: &Path) -> Result<serde_json::Value> {
    let json_path = config_dir.join("rules.json");
    if !json_path.exists() {
        return Ok(serde_json::Value::Array(vec![]));
    }
    let content = std::fs::read_to_string(&json_path)
        .with_context(|| format!("读取 rules.json 失败: {}", json_path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("解析 rules.json 失败: {}", json_path.display()))
}
