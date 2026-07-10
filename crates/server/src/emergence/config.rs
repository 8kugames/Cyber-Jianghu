// ============================================================================
// 涌现检测配置类型（emergence.yaml 反序列化）
// ============================================================================
//
// 全部阈值与动作映射由 emergence.yaml 驱动，判定逻辑零硬编码。
// 配置缺失时 fail-fast（与 server config 哲学一致）。
// ============================================================================

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 涌现检测完整配置（对应 emergence.yaml 顶层）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmergenceConfig {
    pub version: String,
    pub detection: DetectionConfig,
    pub causal: CausalConfig,
    #[serde(default)]
    pub health: HealthConfig,
}

/// 形态筛选配置（阶段 1）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionConfig {
    /// 动作 → 社会类别映射规则（数据驱动）
    pub category_rules: BTreeMap<String, CategoryRule>,
    pub min_agents: usize,
    pub min_actions: usize,
    pub min_categories: usize,
    pub chain_gap_ticks: i64,
    #[serde(default = "default_max_events")]
    pub max_events: usize,
}

fn default_max_events() -> usize {
    50
}

/// 单条社会类别规则（两种形态：actions 列表 / transfer_actions 列表）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CategoryRule {
    /// 形态 A：按动作名列表 + 可选 success/target 约束
    Actions {
        actions: Vec<String>,
        #[serde(default)]
        require_success: bool,
        #[serde(default)]
        require_target: bool,
    },
    /// 形态 B：物品转移类（予/取 方向字段判定，支持多个 transfer 原语）
    TransferActions {
        transfer_actions: Vec<TransferSpec>,
    },
}

/// 单个转移规则（action + 方向字段判定）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferSpec {
    pub action: String,
    pub direction_field: String,
    pub direction_value: String,
}

/// 因果验证配置（阶段 2）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CausalConfig {
    pub min_causal_edges: usize,
    #[serde(default = "default_match_by")]
    pub match_by: Vec<String>,
    #[serde(default = "default_short_uuid_len")]
    pub short_uuid_len: usize,
}

fn default_match_by() -> Vec<String> {
    vec!["name".to_string(), "short_uuid".to_string()]
}

fn default_short_uuid_len() -> usize {
    8
}

/// MVP §6.1.2 生存能力配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HealthConfig {
    #[serde(default)]
    pub supply_actions: Vec<String>,
    #[serde(default = "default_min_survivors")]
    pub min_survivors: i32,
    #[serde(default = "default_min_supply_count")]
    pub min_supply_count: i32,
}

fn default_min_survivors() -> i32 {
    3
}

fn default_min_supply_count() -> i32 {
    3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_parses_yaml() {
        // 测试 actions 形态
        let actions_rule: CategoryRule = serde_yaml::from_str(
            r#"
            actions: [攻击]
            require_success: true
            require_target: true
            "#,
        )
        .unwrap();
        match actions_rule {
            CategoryRule::Actions {
                actions,
                require_success,
                require_target,
            } => {
                assert_eq!(actions, vec!["攻击"]);
                assert!(require_success);
                assert!(require_target);
            }
            _ => panic!("应为 Actions 形态"),
        }

        // 测试 transfer_actions 形态（与实际 emergence.yaml 格式一致）
        let transfer_rule: CategoryRule = serde_yaml::from_str(
            r#"
            transfer_actions:
              - action: 予
                direction_field: recipient_type
                direction_value: agent
              - action: 取
                direction_field: source_type
                direction_value: agent
            "#,
        )
        .unwrap();
        match transfer_rule {
            CategoryRule::TransferActions { transfer_actions } => {
                assert_eq!(transfer_actions.len(), 2);
                assert_eq!(transfer_actions[0].action, "予");
                assert_eq!(transfer_actions[0].direction_field, "recipient_type");
                assert_eq!(transfer_actions[0].direction_value, "agent");
                assert_eq!(transfer_actions[1].action, "取");
            }
            _ => panic!("应为 TransferActions 形态"),
        }
    }

    #[test]
    fn test_full_config_parses() {
        let yaml = r#"
version: "1.0"
detection:
  category_rules:
    conflict:
      actions: [攻击]
      require_success: true
      require_target: true
    trade:
      transfer_actions:
        - action: 予
          direction_field: recipient_type
          direction_value: agent
  min_agents: 2
  min_actions: 3
  min_categories: 2
  chain_gap_ticks: 5
causal:
  min_causal_edges: 1
  match_by: [name, short_uuid]
  short_uuid_len: 8
health:
  supply_actions: [用]
  min_survivors: 3
  min_supply_count: 3
"#;
        let cfg: EmergenceConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.version, "1.0");
        assert_eq!(cfg.detection.min_agents, 2);
        assert_eq!(cfg.detection.category_rules.len(), 2);
        assert!(cfg.detection.category_rules.contains_key("conflict"));
        assert!(cfg.detection.category_rules.contains_key("trade"));
        assert_eq!(cfg.causal.min_causal_edges, 1);
        assert_eq!(cfg.health.min_survivors, 3);
    }
}

#[cfg(test)]
mod real_yaml_tests {
    use super::*;

    #[test]
    fn test_real_config_file_parses() {
        // 用真实的 config/emergence.yaml 实测，而非手写片段
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("config/emergence.yaml");
        if !path.exists() {
            eprintln!("跳过：{} 不存在", path.display());
            return;
        }
        let cfg: EmergenceConfig = crate::game_data::loaders::config_format::load_config(&path)
            .expect("真实 emergence.yaml 必须能被 EmergenceConfig 解析");
        assert!(!cfg.detection.category_rules.is_empty(), "category_rules 不能为空");
        // 验证每条规则都正确解析（不是被吞成 None）
        for (name, rule) in &cfg.detection.category_rules {
            eprintln!("规则 {}: 解析为 {:?}", name, rule);
        }
        // 特别验证 trade 规则（transfer_actions 格式）
        assert!(cfg.detection.category_rules.contains_key("trade"), "trade 规则必须存在");
        assert!(cfg.detection.category_rules.contains_key("conflict"), "conflict 规则必须存在");
        assert!(cfg.detection.category_rules.contains_key("communication"), "communication 规则必须存在");
    }
}
