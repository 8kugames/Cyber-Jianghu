//! SFT transform 纯函数
//!
//! 契约对齐 scripts/build_sft_data.py:158-197.
//! 天魂筛选 (attempt 精确匹配) 在 runner 层做, 本模块只做单条 trace → SftSample 转换.

use cyber_jianghu_protocol::TraceEntry;
use serde::{Deserialize, Serialize};

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
        content: response.to_string(),
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
