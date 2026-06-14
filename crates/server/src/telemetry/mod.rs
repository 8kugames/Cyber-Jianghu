use std::time::Duration;

pub mod collector;
pub mod storage;

use crate::DbPool;

/// 遥测聚合配置（对应 telemetry_config.yaml）
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TelemetryConfig {
    /// 全局默认聚合间隔（分钟）
    pub aggregation_interval_minutes: u64,
    /// 聚合定义列表
    pub aggregations: Vec<AggregationConfig>,
}

/// 单个聚合的配置
#[derive(Debug, Clone, serde::Deserialize)]
pub struct AggregationConfig {
    pub name: String,
    pub description: String,
    pub event_source: String,
    /// 可选：覆盖全局间隔（分钟）
    #[serde(default)]
    pub period_minutes: Option<u64>,
    /// 分组字段（空列表 = 全量统计）
    #[serde(default)]
    pub group_by: Vec<String>,
    /// 指标列表
    pub metrics: Vec<String>,
    /// 可选：交互 partner 的 JSONB key 列表（仅 interaction_activity 使用）
    #[serde(default)]
    pub jsonb_partner_fields: Vec<String>,
}

/// 聚合结果行
#[derive(Debug, Clone, serde::Serialize)]
pub struct TelemetryRow {
    pub id: i64,
    pub aggregation_name: String,
    pub period_start: chrono::DateTime<chrono::Utc>,
    pub period_end: chrono::DateTime<chrono::Utc>,
    pub group_by_key: Option<String>,
    pub group_by_value: Option<String>,
    pub metrics: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// 聚合指标（序列化用）
#[derive(Debug, Clone, serde::Serialize)]
pub struct AggregationMetrics {
    pub count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_duration_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p50_duration_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p95_duration_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unique_interacting_agents: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_count: Option<i64>,
}

/// 加载遥测配置
pub fn load_config() -> Result<TelemetryConfig, String> {
    let config_dir = crate::paths::get_config_dir();
    let path = config_dir.join("telemetry_config.yaml");
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取 telemetry_config.yaml 失败: {}", e))?;
    let config: TelemetryConfig = serde_yaml::from_str(&content)
        .map_err(|e| format!("解析 telemetry_config.yaml 失败: {}", e))?;
    Ok(config)
}

/// 启动遥测采集器（异步定时任务）
/// 每个聚合定义各自启动一个独立 tokio task，按配置间隔运行
/// 返回所有 handle，调用方应加入 select! 以支持 graceful shutdown
pub fn start_telemetry_collector(db_pool: DbPool) -> Vec<tokio::task::JoinHandle<()>> {
    let config = match load_config() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("遥测配置加载失败，遥测功能不可用: {}", e);
            return Vec::new();
        }
    };

    let mut handles = Vec::with_capacity(config.aggregations.len());
    for agg in config.aggregations {
        let interval_minutes = agg
            .period_minutes
            .unwrap_or(config.aggregation_interval_minutes);
        let db = db_pool.clone();
        let agg_name = agg.name.clone();
        let event_source = agg.event_source.clone();
        let group_by = agg.group_by.clone();
        let metrics = agg.metrics.clone();
        let partner_fields = agg.jsonb_partner_fields.clone();

        let period_minutes = interval_minutes;
        handles.push(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(interval_minutes * 60));
            // 推迟首个周期，给 server 启动留出缓冲
            interval.tick().await;

            loop {
                interval.tick().await;
                if let Err(e) = collector::run_aggregation(
                    &db,
                    &agg_name,
                    &event_source,
                    &group_by,
                    &metrics,
                    &partner_fields,
                    period_minutes,
                )
                .await
                {
                    tracing::warn!("遥测聚合 {} 失败: {}", agg_name, e);
                }
            }
        }));
    }
    handles
}
