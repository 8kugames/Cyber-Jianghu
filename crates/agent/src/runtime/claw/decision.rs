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

const DEFAULT_DECISION_RULES: &str = r#"决策规则：
1. 优先满足生理需求（饥饿、口渴）
2. 如果状态良好，可以考虑探索、社交或赚钱
3. 保持角色人设一致
4. 只返回 JSON 格式的决策结果

决策格式：
{"action_type": "动作类型", "action_data": {"参数": "值"}, "thought": "思考过程"}"#;

impl ClawDecisionState {
    pub fn new(llm: Arc<dyn LlmClient>) -> Self {
        Self {
            llm,
            context_builder: ContextBuilder::new(),
            system_prompt: format!(
                "你是一个武侠游戏中的角色。你需要根据当前状态做出合理的决策。\n\n{}",
                DEFAULT_DECISION_RULES
            ),
        }
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = format!("{}\n\n{}", prompt.into(), DEFAULT_DECISION_RULES);
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

#[cfg(test)]
mod tests {
    #[test]
    fn test_parse_missing_action_type() {
        let json = r#"{"action_data": {}, "thought": "test"}"#;
        let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
        
        let action_type = parsed
            .get("action_type")
            .and_then(|v| v.as_str())
            .unwrap_or("idle");
        
        assert_eq!(action_type, "idle");
    }

    #[test]
    fn test_parse_malformed_json_fallback() {
        let response = "This is not JSON at all";
        
        let json_start = response.find('{');
        
        assert!(json_start.is_none());
    }

    #[test]
    fn test_extract_json_from_text() {
        let response = "Sure, here is the decision:\n{\"action_type\": \"idle\"}\n\nLet me know if you need anything else.";
        
        let json_start = response.find('{');
        let json_end = response.rfind('}').map(|p| p + 1);
        
        let json_str = match (json_start, json_end) {
            (Some(start), Some(end)) => &response[start..=end],
            _ => "",
        };
        
        let parsed: serde_json::Value = serde_json::from_str(json_str).unwrap();
        assert_eq!(parsed["action_type"], "idle");
    }
}

pub fn create_claw_decision_callback(
    state: ClawDecisionState,
) -> Arc<dyn Fn(&WorldState) -> std::pin::Pin<Box<dyn std::future::Future<Output = Intent> + Send>> + Send + Sync> {
    let state = Arc::new(state);
    
    Arc::new(move |world_state: &WorldState| {
        let state = state.clone();
        let world_state = world_state.clone();
        Box::pin(async move {
            match claw_decision(&state, &world_state).await {
                Ok(intent) => intent,
                Err(e) => {
                    error!("Claw decision failed: {}", e);
                    Intent::idle(
                        world_state.agent_id.unwrap_or_default(),
                        world_state.tick_id,
                    )
                }
            }
        })
    })
}
