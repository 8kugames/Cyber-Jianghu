// ============================================================================
// OpenClaw Cyber-Jianghu 位置配置加载器
// ============================================================================
//
// 本模块负责加载位置配置（locations.yaml 或 locations.json）
// ============================================================================

use crate::game_data::loaders::config_format::load_config;
use crate::game_data::types::UnifiedLocationsConfig;
use anyhow::{Context, Result};
use std::path::Path;

/// 加载位置配置
///
/// 优先加载 YAML 格式，回退到 JSON 格式。
///
/// # 参数
/// - `config_dir`: 配置文件目录路径
///
/// # 返回
/// 统一位置配置对象
pub fn load_locations<P: AsRef<Path>>(config_dir: P) -> Result<UnifiedLocationsConfig> {
    let config_dir = config_dir.as_ref();

    // 优先尝试 YAML 格式
    let yaml_path = config_dir.join("locations.yaml");
    if yaml_path.exists() {
        return load_config(&yaml_path).context("加载位置配置 (YAML) 失败");
    }

    // 回退到 JSON 格式
    let json_path = config_dir.join("locations.json");
    load_config(&json_path).context("加载位置配置 (JSON) 失败")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_data::loaders::config_format::{ConfigFormat, parse_config};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_locations_json() {
        let dir = TempDir::new().unwrap();

        // 创建测试配置文件
        fs::write(
            dir.path().join("locations.json"),
            r#"{
                "version": "2.0.0",
                "description": "位置配置文件",
                "meta": {},
                "data": {
                    "nodes": [
                        {
                            "node_id": "inn",
                            "name": "龙门客栈",
                            "type": "map",
                            "parent_id": "河西走廊"
                        },
                        {
                            "node_id": "lobby",
                            "name": "大堂",
                            "type": "sub_scene",
                            "parent_id": "inn"
                        }
                    ],
                    "edges": []
                }
            }"#,
        )
        .unwrap();

        let config = load_locations(dir.path()).unwrap();
        assert_eq!(config.version, "2.0.0");
        assert_eq!(config.data.nodes.len(), 2);
        assert_eq!(config.data.nodes[0].node_id, "inn");
        assert_eq!(config.data.nodes[0].name, "龙门客栈");
        assert_eq!(config.data.edges.len(), 0);
    }

    #[test]
    fn test_load_locations_yaml() {
        let yaml = r#"
version: "2.0.0"
description: "位置配置文件"
meta: {}
data:
  nodes:
    - node_id: "inn"
      name: "龙门客栈"
      type: "map"
      parent_id: "河西走廊"
    - node_id: "lobby"
      name: "大堂"
      type: "sub_scene"
      parent_id: "inn"
  edges: []
"#;

        let config: UnifiedLocationsConfig = parse_config(yaml, ConfigFormat::Yaml).unwrap();
        assert_eq!(config.version, "2.0.0");
        assert_eq!(config.data.nodes.len(), 2);
        assert_eq!(config.data.nodes[0].node_id, "inn");
    }
}
