//! SFT transform 纯函数
//!
//! 契约对齐 scripts/build_sft_data.py:158-197.
//! 天魂筛选 (attempt 精确匹配) 在 runner 层做, 本模块只做单条 trace → SftSample 转换.

use cyber_jianghu_protocol::TraceEntry;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// actions.yaml v2.0 改名前的历史动作名 → 现行名。
///
/// 存量人魂 trace（改名前采集）含旧动作名，直接导出会把旧名教给微调模型，
/// 压低动作类型合法率。归一仅发生在导出转换层，不改写落盘的原始 trace。
const LEGACY_ACTION_ALIASES: &[(&str, &str)] = &[
    ("进食", "吃"),
    ("饮水", "喝"),
    ("拾取", "取"),
    ("使用", "用"),
    ("给予", "予"),
    ("打坐", "休整"),
    ("修炼", "休整"),
    ("休息", "休整"),
    ("私语", "说话"),
];

/// 地魂工具调用残留（非动作意图，无对应 v2.0 动作），导出时整条丢弃。
const TOOL_TRACE_ACTION_TYPES: &[&str] = &["query_world", "查询状态", "检查背包"];

/// 归一 assistant response 中的历史动作名，丢弃工具调用残留。
///
/// - 严格 JSON 解析失败、或无 `actions` 数组：原样透传（fail-open，
///   纯文本 response 一直是合法导出形态）。
/// - 丢弃导致 actions 从非空变空：返回 None（空动作样本无训练价值）。
/// - JSON 重新序列化后字段顺序变为字母序（serde_json 默认），语义不变。
fn normalize_action_types(response: &str) -> Option<String> {
    let Ok(mut value) = serde_json::from_str::<Value>(response) else {
        return Some(response.to_string());
    };
    let Some(actions) = value.get_mut("actions").and_then(Value::as_array_mut) else {
        return Some(response.to_string());
    };
    let was_nonempty = !actions.is_empty();
    actions.retain(|action| {
        action
            .get("action_type")
            .and_then(Value::as_str)
            .map(|t| !TOOL_TRACE_ACTION_TYPES.contains(&t))
            .unwrap_or(true)
    });
    for action in actions.iter_mut() {
        let Some(current) = action.get("action_type").and_then(Value::as_str) else {
            continue;
        };
        if let Some(&(_, canonical)) = LEGACY_ACTION_ALIASES
            .iter()
            .find(|(legacy, _)| *legacy == current)
            && let Some(obj) = action.as_object_mut()
        {
            obj.insert(
                "action_type".to_string(),
                Value::String(canonical.to_string()),
            );
        }
    }
    if was_nonempty && actions.is_empty() {
        return None;
    }
    Some(value.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SftSample {
    pub messages: Vec<SftMessage>,
    pub metadata: SftSampleMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SftMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SftSampleMetadata {
    pub agent_id: String,
    pub tick_id: i64,
    pub soul_stage: String,
    pub attempt: i32,
    pub provider: String,
    pub model: String,
    /// None = 未筛选 (对应 Python --no-db-filter 模式的 null);
    /// Some("approved") = 天魂审查通过; Some("rejected") = 审查驳回 (当前不会导出).
    /// 对齐 ADV-01: 用 Option<String> 与 Python 基线一致, 不用 "no_filter" sentinel.
    pub tianhun_result: Option<String>,
    pub trace_id: String,
}

pub struct TransformInput<'a> {
    pub entry: &'a TraceEntry,
    pub tianhun_result: Option<String>,
}

pub fn transform_entry(input: TransformInput<'_>) -> Option<SftSample> {
    let entry = input.entry;
    let response = entry.response.trim();
    if response.is_empty() || !entry.ok {
        return None;
    }

    let mut messages: Vec<SftMessage> = Vec::with_capacity(3);
    let persona_name = entry.persona_name.trim();
    if !persona_name.is_empty() {
        let mut system_content = format!("你是 {}。", persona_name);
        let persona_desc = entry.persona_description.trim();
        if !persona_desc.is_empty() {
            system_content.push('\n');
            system_content.push_str(persona_desc);
        }
        messages.push(SftMessage {
            role: "system".to_string(),
            content: system_content,
        });
    }
    messages.push(SftMessage {
        role: "user".to_string(),
        content: entry.user_prompt.clone(),
    });
    messages.push(SftMessage {
        role: "assistant".to_string(),
        content: normalize_action_types(response)?,
    });

    Some(SftSample {
        messages,
        metadata: SftSampleMetadata {
            agent_id: entry.agent_id.to_string(),
            tick_id: entry.tick_id,
            soul_stage: entry.soul_stage.clone(),
            attempt: entry.attempt,
            provider: entry.provider.clone(),
            model: entry.model.clone(),
            tianhun_result: input.tianhun_result,
            trace_id: entry.trace_id.clone(),
        },
    })
}
