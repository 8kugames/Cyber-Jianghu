// Claw Decision - 内部调度器决策函数

use std::sync::Arc;
use anyhow::Result;
use tracing::{debug, error, info};

use crate::ai::llm::LlmClient;
use crate::models::{Intent, WorldState};
use crate::runtime::claw::ContextBuilder;

pub struct ClawDecisionState {
    pub llm: Arc<dyn LlmClient>,
    pub context_builder: ContextBuilder,
    pub system_prompt: String,
}

impl ClawDecisionState {
    pub fn new(llm: Arc<dyn LlmClient>) -> Self {
        Self {
            llm,
            context_builder: ContextBuilder::new(),
            system_prompt: r#"你是一个武侠游戏中的角色。你需要根据当前状态做出合理的决策。

决策规则：
1. 优先满足生理需求（饥饿、口渴）
2. 如果状态良好，可以考虑探索、社交或赚钱
3. 保持角色人设一致
4. 只返回 JSON 格式的决策结果

决策格式：
{"action_type": "动作类型", "action_data": {"参数": "值"}, "thought": "思考过程"}"#.to_string(),
        }
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }
}

pub async fn claw_decision(
    state: &ClawDecisionState,
    world_state: &WorldState,
) -> Result<Intent> {
    let tick_id = world_state.tick_id;
    let agent_id = world_state.agent_id.unwrap_or_default();
    
    let context = state.context_builder.build(world_state);
    debug!("Context for tick {}: {} chars", tick_id, context.len());
    
    let prompt = format!(
        "[系统]\n{}\n\n[当前状态]\n{}\n\n[助手]",
        state.system_prompt,
        context
    );
    
    let response = state.llm.complete(&prompt).await
        .map_err(|e| anyhow::anyhow!("LLM call failed: {}", e))?;
    
    debug!("LLM response: {} chars", response.len());
    
    let json_start = response.find('{');
    let json_end = response.rfind('}').map(|p| p + 1);
    
    let json_str = match (json_start, json_end) {
        (Some(start), Some(end)) => &response[start..end],
        _ => {
            error!("No JSON found in LLM response");
            return Ok(Intent::idle(agent_id, tick_id));
        }
    };
    
    let parsed: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(e) => {
            error!("Failed to parse JSON: {}", e);
            return Ok(Intent::idle(agent_id, tick_id));
        }
    };
    
    let action_type = parsed
        .get("action_type")
        .and_then(|v| v.as_str())
        .unwrap_or("idle");
    
    let action_data = parsed.get("action_data").cloned();
    let thought = parsed
        .get("thought")
        .and_then(|v| v.as_str())
        .map(String::from);
    
    info!("Claw decision for tick {}: {}", tick_id, action_type);
    
    let mut intent = Intent::new(agent_id, tick_id, action_type, action_data);
    if let Some(t) = thought {
        intent = intent.with_thought(t);
    }
    Ok(intent)
}

pub fn create_claw_decision_callback(
    state: ClawDecisionState,
) -> impl Fn(&WorldState) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Intent>> + Send>> + Send + Sync + 'static {
    let state = Arc::new(state);
    
    move |world_state: &WorldState| {
        let state = state.clone();
        let world_state = world_state.clone();
        Box::pin(async move {
            claw_decision(&state, &world_state).await
        })
    }
}
