//! 遥测系统集成测试
//!
//! 测试 telemetry_config.yaml 配置解析、数据结构序列化等。
//! 完整的 DB 端到端测试需要数据库连接和服务状态，在此仅做烟雾测试。
//!
//!
//! ## 测试缺口 (Finding 10)
//! - Collector SQL 查询无 DB 集成测试（需运行 PostgreSQL）
//! - API handler 无 HTTP 集成测试
//! - `get_latest_aggregation_time` / `query_aggregations` 无存储层测试
//!
//! 以上需在 CI 有 DB 环境时补充。

#[cfg(test)]
mod tests {
    use cyber_jianghu_server::telemetry::{AggregationConfig, load_config};

    /// 烟雾测试：验证 telemetry_config.yaml 能正确解析
    #[test]
    fn test_load_config_success() {
        let config = load_config().expect("telemetry_config.yaml 应能正常加载");
        assert_eq!(config.aggregation_interval_minutes, 60);
        assert_eq!(config.aggregations.len(), 5, "应有 5 种聚合定义");
    }

    /// 验证每个聚合定义的必要字段完整
    #[test]
    fn test_aggregation_definitions_complete() {
        let config = load_config().expect("配置加载");
        for agg in &config.aggregations {
            assert!(!agg.name.is_empty(), "聚合名称不能为空");
            assert!(!agg.description.is_empty(), "聚合描述不能为空");
            assert!(!agg.event_source.is_empty(), "event_source 不能为空");
            assert!(!agg.metrics.is_empty(), "metrics 不能为空");
        }
    }

    /// 验证 survival_time 聚合
    #[test]
    fn test_survival_time_aggregation() {
        let agg = find_agg("survival_time");
        assert_eq!(agg.event_source, "agents");
        assert!(agg.group_by.is_empty(), "survival_time 不应有分组");
        assert!(agg.metrics.contains(&"count".to_string()));
        assert!(agg.metrics.contains(&"avg_duration_seconds".to_string()));
        assert!(agg.metrics.contains(&"p50_duration_seconds".to_string()));
        assert!(agg.metrics.contains(&"p95_duration_seconds".to_string()));
    }

    /// 验证 decision_distribution 聚合
    #[test]
    fn test_decision_distribution_aggregation() {
        let agg = find_agg("decision_distribution");
        assert_eq!(agg.event_source, "agent_action_logs");
        assert!(agg.group_by.contains(&"action_type".to_string()));
        assert!(agg.metrics.contains(&"count".to_string()));
        assert!(agg.metrics.contains(&"success_rate".to_string()));
    }

    /// 验证 action_outcomes 聚合
    #[test]
    fn test_action_outcomes_aggregation() {
        let agg = find_agg("action_outcomes");
        assert_eq!(agg.event_source, "agent_action_logs");
        assert!(agg.group_by.contains(&"result".to_string()));
        assert!(agg.metrics.contains(&"count".to_string()));
    }

    /// 验证 interaction_activity 聚合（每日间隔）
    #[test]
    fn test_interaction_activity_aggregation() {
        let agg = find_agg("interaction_activity");
        assert_eq!(agg.event_source, "agent_action_logs");
        assert_eq!(agg.period_minutes, Some(1440), "交互活跃度应为每日聚合");
        assert!(agg.group_by.is_empty());
        assert!(agg.metrics.contains(&"action_count".to_string()));
        assert!(
            agg.metrics
                .contains(&"unique_interacting_agents".to_string())
        );
    }

    /// 验证 location_traffic 聚合
    #[test]
    fn test_location_traffic_aggregation() {
        let agg = find_agg("location_traffic");
        assert_eq!(agg.event_source, "agent_states");
        assert!(agg.group_by.contains(&"node_id".to_string()));
        assert!(agg.metrics.contains(&"agent_count".to_string()));
        assert!(agg.metrics.contains(&"state_count".to_string()));
    }

    /// 验证 TelemetryConfig 的 Deserialize 和 Debug trait
    #[test]
    fn test_config_traits() {
        let config = load_config().expect("配置加载");
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("TelemetryConfig"));
        assert!(debug_str.contains("survival_time"));
        let _cloned = config.clone();
    }

    /// 验证 aggregation_interval_minutes 默认值被各聚合正确继承
    #[test]
    fn test_period_minutes_inheritance() {
        let config = load_config().expect("配置加载");
        for agg in &config.aggregations {
            let effective = agg
                .period_minutes
                .unwrap_or(config.aggregation_interval_minutes);
            assert!(effective > 0, "有效周期必须 > 0");
        }
    }

    /// 验证 event_source 只包含有效值
    #[test]
    fn test_event_source_valid() {
        let config = load_config().expect("配置加载");
        let valid_sources = ["agents", "agent_action_logs", "agent_states"];
        for agg in &config.aggregations {
            assert!(
                valid_sources.contains(&agg.event_source.as_str()),
                "event_source '{}' 不在合法列表中",
                agg.event_source
            );
        }
    }

    // ── 辅助 ──────────────────────────────────────────────────────────────

    fn find_agg(name: &str) -> AggregationConfig {
        let config = load_config().expect("配置加载");
        config
            .aggregations
            .into_iter()
            .find(|a| a.name == name)
            .unwrap_or_else(|| panic!("未找到聚合 '{}'", name))
    }
}
