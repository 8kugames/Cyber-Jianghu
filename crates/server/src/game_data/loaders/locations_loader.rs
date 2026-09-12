// ============================================================================
// OpenClaw Cyber-Jianghu 位置配置加载器
// ============================================================================
//
// 本模块负责加载位置配置（locations.yaml 或 locations.json），并做
// 图引用完整性校验（fail-fast：坏图拒绝启动，而非静默带病运行）。
//
// 完整图不变量（连通性、非对称边画像、可达性预算）见
// crates/server/tests/locations_graph_integrity_test.rs（CI 数据画像守卫）。
// ============================================================================

use crate::game_data::loaders::config_format::load_config;
use crate::game_data::types::UnifiedLocationsConfig;
use anyhow::{Context, Result};
use cyber_jianghu_protocol::LocationNodeType;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// 加载位置配置
///
/// 优先加载 YAML 格式，回退到 JSON 格式；加载后执行图引用完整性校验。
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
    let config = if yaml_path.exists() {
        load_config(&yaml_path).context("加载位置配置 (YAML) 失败")?
    } else {
        // 回退到 JSON 格式
        let json_path = config_dir.join("locations.json");
        load_config(&json_path).context("加载位置配置 (JSON) 失败")?
    };

    validate_locations(&config)?;
    Ok(config)
}

/// 图引用完整性校验（fail-fast）
///
/// - 边端点必须引用已定义节点（悬空边 = 断头路，直接报错）
/// - parent_id 必须引用已定义节点（层级引用悬空 = 孤儿子场景）
/// - 根节点类型必须为 region（地图分层模型 Region → Map → SubScene；
///   WorldState.parent_chain 链首恒为区域级、agent 端层级表述去首级的假设依赖此不变量）
/// - 父链不得成环（成环会让祖先链上溯悬挂/错乱）
/// - time_variants 闭区间必须自洽（from_game_day <= to_game_day）
/// - 非对称边（A→B 有、B→A 无）是合法特性（如"回程更累"），仅告警提示
pub fn validate_locations(config: &UnifiedLocationsConfig) -> Result<()> {
    let node_ids: HashSet<&str> = config
        .data
        .nodes
        .iter()
        .map(|n| n.node_id.as_str())
        .collect();

    // 重复 node_id 会让 HashMap 静默覆盖，先拦下
    if node_ids.len() != config.data.nodes.len() {
        let mut seen = HashSet::new();
        let dupes: Vec<&str> = config
            .data
            .nodes
            .iter()
            .filter(|n| !seen.insert(n.node_id.as_str()))
            .map(|n| n.node_id.as_str())
            .collect();
        anyhow::bail!("位置配置存在重复 node_id: {}", dupes.join(", "));
    }

    for node in &config.data.nodes {
        if let Some(parent) = &node.parent_id
            && !parent.is_empty()
            && !node_ids.contains(parent.as_str())
        {
            anyhow::bail!(
                "节点 {} 的 parent_id 引用不存在的节点: {}",
                node.node_id,
                parent
            );
        }
        if node.parent_id.as_deref().unwrap_or("").is_empty()
            && node.node_type != LocationNodeType::Region
        {
            anyhow::bail!(
                "根节点 {} 的类型必须为 region（地图分层模型 Region → Map → SubScene），当前为 {}",
                node.node_id,
                node.node_type
            );
        }
        for (i, v) in node.time_variants.iter().enumerate() {
            if let (Some(from), Some(to)) = (v.from_game_day, v.to_game_day)
                && from > to
            {
                anyhow::bail!(
                    "节点 {} 的 time_variants[{}] 区间无效: from_game_day={} > to_game_day={}",
                    node.node_id,
                    i,
                    from,
                    to
                );
            }
        }
    }

    for edge in &config.data.edges {
        if !node_ids.contains(edge.from.as_str()) {
            anyhow::bail!(
                "边的 from_node_id 引用不存在的节点: {} → {}",
                edge.from,
                edge.to
            );
        }
        if !node_ids.contains(edge.to.as_str()) {
            anyhow::bail!(
                "边的 to_node_id 引用不存在的节点: {} → {}",
                edge.from,
                edge.to
            );
        }
    }

    // 父链成环会让任何上溯逻辑（WorldState.parent_chain、dashboard 层级）悬挂/错乱，
    // 环是非法树结构，坏图拒绝启动（与悬空 parent 同等对待）
    let nodes_by_id: HashMap<&str, &cyber_jianghu_protocol::LocationNode> = config
        .data
        .nodes
        .iter()
        .map(|n| (n.node_id.as_str(), n))
        .collect();
    for node in &config.data.nodes {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut cursor = node.parent_id.as_deref().filter(|p| !p.is_empty());
        while let Some(pid) = cursor {
            if !seen.insert(pid) {
                anyhow::bail!("位置父链成环: {} 的祖先链在 {} 处闭环", node.node_id, pid);
            }
            cursor = nodes_by_id
                .get(pid)
                .and_then(|n| n.parent_id.as_deref())
                .filter(|p| !p.is_empty());
        }
    }

    // 非对称边画像：合法但需显式可见（CI 守卫固化清单，防止手误断头路）
    let mut declared: HashMap<(&str, &str), i32> = HashMap::new();
    for edge in &config.data.edges {
        declared.insert((edge.from.as_str(), edge.to.as_str()), edge.travel_cost);
    }
    for ((from, to), cost) in &declared {
        if !declared.contains_key(&(*to, *from)) {
            tracing::warn!(
                "非对称边（无回程边，若非有意请补反方向）: {} → {} (travel_cost={})",
                from,
                to,
                cost
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_data::loaders::config_format::{ConfigFormat, parse_config};
    use std::fs;
    use tempfile::TempDir;

    fn base_yaml(extra_nodes: &str, extra_edges: &str) -> String {
        format!(
            r#"
version: "1.0"
description: "测试"
meta: {{}}
data:
  nodes:
    - node_id: "inn"
      name: "客栈"
      type: "region"
      parent_id: ""
    - node_id: "lobby"
      name: "大堂"
      type: "sub_scene"
      parent_id: "inn"
{extra_nodes}
  edges:
    - from_node_id: "inn"
      to_node_id: "lobby"
      travel_cost: 1
{extra_edges}
"#
        )
    }

    fn parse(yaml: &str) -> Result<UnifiedLocationsConfig> {
        parse_config(yaml, ConfigFormat::Yaml).context("解析失败")
    }

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
                            "node_id": "河西走廊",
                            "name": "河西走廊",
                            "type": "region",
                            "parent_id": ""
                        },
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
        assert_eq!(config.data.nodes.len(), 3);
        assert_eq!(config.data.nodes[1].node_id, "inn");
        assert_eq!(config.data.nodes[1].name, "龙门客栈");
        assert_eq!(config.data.edges.len(), 0);
    }

    #[test]
    fn test_load_locations_yaml() {
        let config = load_locations(valid_config_dir()).unwrap();
        assert_eq!(config.version, "2.0.0");
        assert_eq!(config.data.nodes.len(), 3);
        assert_eq!(config.data.nodes[1].node_id, "inn");
    }

    fn valid_config_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("locations.yaml"),
            r#"
version: "2.0.0"
description: "位置配置文件"
meta: {}
data:
  nodes:
    - node_id: "河西走廊"
      name: "河西走廊"
      type: "region"
      parent_id: ""
    - node_id: "inn"
      name: "龙门客栈"
      type: "map"
      parent_id: "河西走廊"
    - node_id: "lobby"
      name: "大堂"
      type: "sub_scene"
      parent_id: "inn"
  edges: []
"#,
        )
        .unwrap();
        dir
    }

    #[test]
    fn test_validate_rejects_dangling_edge_endpoint() {
        let yaml = base_yaml(
            "",
            r#"    - from_node_id: "inn"
      to_node_id: "ghost_town"
      travel_cost: 3"#,
        );
        let config = parse(&yaml).unwrap();
        let err = validate_locations(&config).unwrap_err();
        assert!(
            err.to_string().contains("ghost_town"),
            "应拒绝悬空边端点: {err}"
        );
    }

    #[test]
    fn test_validate_rejects_dangling_parent_id() {
        let yaml = base_yaml(
            r#"    - node_id: "orphan"
      name: "孤儿"
      type: "sub_scene"
      parent_id: "nowhere""#,
            "",
        );
        let config = parse(&yaml).unwrap();
        let err = validate_locations(&config).unwrap_err();
        assert!(
            err.to_string().contains("nowhere"),
            "应拒绝悬空 parent_id: {err}"
        );
    }

    #[test]
    fn test_validate_rejects_duplicate_node_id() {
        let yaml = base_yaml(
            r#"    - node_id: "inn"
      name: "重复客栈"
      type: "map"
      parent_id: """#,
            "",
        );
        let config = parse(&yaml).unwrap();
        let err = validate_locations(&config).unwrap_err();
        assert!(
            err.to_string().contains("重复 node_id"),
            "应拒绝重复 node_id: {err}"
        );
    }

    #[test]
    fn test_validate_rejects_inverted_time_variant_interval() {
        let yaml = base_yaml(
            r#"    - node_id: "ruins"
      name: "废墟"
      type: "map"
      parent_id: "inn"
      time_variants:
        - from_game_day: 100
          to_game_day: 50
          visible: false"#,
            "",
        );
        let config = parse(&yaml).unwrap();
        let err = validate_locations(&config).unwrap_err();
        assert!(
            err.to_string().contains("time_variants[0] 区间无效"),
            "应拒绝倒置区间: {err}"
        );
    }

    #[test]
    fn test_validate_accepts_asymmetric_edges() {
        // 非对称边是合法特性（回程更累）：base_yaml 仅声明 inn→lobby 无反向，
        // 校验应通过（tracing 告警，不报错）
        let config = parse(&base_yaml("", "")).unwrap();
        assert!(
            validate_locations(&config).is_ok(),
            "单向边不应报错（仅告警）"
        );
    }

    #[test]
    fn test_validate_accepts_well_formed_time_variants() {
        let yaml = base_yaml(
            r#"    - node_id: "ruins"
      name: "废墟"
      type: "map"
      parent_id: "inn"
      time_variants:
        - from_game_day: 1
          to_game_day: 50
          visible: false
        - from_game_day: 200
          description: "后世重建的城镇""#,
            "",
        );
        let config = parse(&yaml).unwrap();
        assert!(validate_locations(&config).is_ok());
    }

    #[test]
    fn test_validate_rejects_non_region_root() {
        // 地图分层模型要求根节点必为 region：非 region 根会使
        // WorldState.parent_chain 链首区域假设失效，坏图拒绝启动
        let yaml = base_yaml(
            r#"    - node_id: "wild"
      name: "荒野"
      type: "map"
      parent_id: """#,
            "",
        );
        let config = parse(&yaml).unwrap();
        let err = validate_locations(&config).unwrap_err();
        assert!(
            err.to_string().contains("wild") && err.to_string().contains("region"),
            "应拒绝非 region 根节点: {err}"
        );
    }

    #[test]
    fn test_validate_rejects_parent_cycle() {
        let yaml = base_yaml(
            r#"    - node_id: "a"
      name: "甲"
      type: "sub_scene"
      parent_id: "b"
    - node_id: "b"
      name: "乙"
      type: "sub_scene"
      parent_id: "a""#,
            "",
        );
        let config = parse(&yaml).unwrap();
        let err = validate_locations(&config).unwrap_err();
        assert!(err.to_string().contains("成环"), "应拒绝父链成环: {err}");
    }
}
