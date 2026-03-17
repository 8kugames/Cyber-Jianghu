// ============================================================================
// OpenClaw Cyber-Jianghu 位置配置加载器
// ============================================================================
//
// 本模块负责加载位置配置（locations.json）
// ============================================================================

use crate::game_data::types::UnifiedLocationsConfig;
use anyhow::{Context, Result};
use std::path::Path;

/// 加载位置配置
///
/// # 参数
/// - `config_dir`: 配置文件目录路径
///
/// # 返回
/// 统一位置配置对象
pub fn load_locations<P: AsRef<Path>>(config_dir: P) -> Result<UnifiedLocationsConfig> {
    let file_path = config_dir.as_ref().join("locations.json");

    let content = std::fs::read_to_string(&file_path)
        .with_context(|| format!("无法读取配置文件: {}", file_path.display()))?;

    let config: UnifiedLocationsConfig = serde_json::from_str(&content)
        .with_context(|| format!("解析配置文件失败: {}", file_path.display()))?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_locations() {
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
                            "parent_id": "hexi_corridor"
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
}
