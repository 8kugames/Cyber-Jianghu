// ============================================================================
// 三魂循环上报记录类型（SoulCycleRecord / ImmediateIntentRecord）
// ============================================================================

use chrono::{DateTime, Utc};

/// 三魂循环记录条目
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SoulCycleRecord {
    pub id: i64,
    pub tick_id: i64,
    pub attempt: i32,
    pub renhun_narrative: Option<String>,
    pub renhun_thought_log: Option<String>,
    pub tianhun_result: Option<String>,
    pub tianhun_layer1_result: Option<String>,
    pub tianhun_layer2_result: Option<String>,
    pub tianhun_layer3_result: Option<String>,
    pub tianhun_reason: Option<String>,
    pub final_intent_id: Option<String>,
    pub final_action_type: Option<String>,
    pub final_action_data: Option<String>,
    pub final_pipeline_json: Option<String>,
    pub route_type: String,
    pub world_time: Option<String>,
    /// 地魂 tool calling 日志（JSON 序列化的 Vec<EarthToolCall>）
    pub earth_tool_calls: Option<String>,
    /// 该次尝试使用的 LLM 模型 ID（用于经历日志展示）
    pub model_id: Option<String>,
    /// 天魂各层审查结果（JSON 数组，数据驱动可扩展）
    /// 格式: [{"layer":"layer1","passed":true,"detail":null}, ...]
    pub tianhun_layers: Option<String>,
    /// Server 执行结果回填（JSON 对象，key=pipe_seq）
    /// 格式: {"0":{"success":true,"error":null,"state_change_summary":"..."}}
    pub server_execution_results: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// 即时意图记录条目
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImmediateIntentRecord {
    pub id: i64,
    pub tick_id: i64,
    pub intent_id: String,
    pub source_narrative: Option<String>,
    pub route_type: String,
    pub action_type: String,
    pub action_data: Option<String>,
    pub speech_content: Option<String>,
    pub send_status: String,
    pub send_error: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// 构建 SQLite IN 子句占位符：`build_in_placeholders(3)` → `"?1,?2,?3"`
pub(crate) fn build_in_placeholders(count: usize) -> String {
    (1..=count)
        .map(|i| format!("?{}", i))
        .collect::<Vec<_>>()
        .join(",")
}
