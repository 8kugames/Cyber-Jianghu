// ============================================================================
// 天魂 — IntentTranslator 实现
// ============================================================================
//
// 单次 LLM 调用，将叙事意图翻译为结构化 Intent JSON。
// Prompt 极简：只有叙事文本 + WorldState 精确数据 + 动作表。
// ============================================================================

use anyhow::Result;
use serde::Deserialize;
use std::sync::Arc;
use tracing::debug;

use crate::component::llm::LlmClientExt;
use crate::infra::api::cognitive_context::load_available_actions_from_file;
use crate::models::Intent;
use crate::models::WorldState;
use crate::soul::actor::CognitiveChain;
use cyber_jianghu_protocol::AvailableAction;

/// LLM 翻译响应（JSON 解析用）
#[derive(Debug, Clone, Deserialize)]
pub struct TranslationResponse {
    /// 服务端动作类型（eat, move, idle, speak...）
    pub action_type: String,
    /// 动作参数（含精确 ID）
    #[serde(default)]
    pub action_data: serde_json::Value,
    /// 提取的说话内容（如果有）
    #[serde(default)]
    pub speech_content: Option<String>,
}

/// 多 Intent 翻译结果
#[derive(Debug)]
pub struct MultiTranslationResult {
    /// 翻译后的 Intent 列表（按执行顺序）
    pub intents: Vec<Intent>,
    /// 即时说话 Intent
    pub speech_intent: Option<Intent>,
    /// 原始叙事文本
    pub original_narrative: String,
    /// 原始思考日志
    pub original_thought_log: String,
}

/// 天魂 — 意图翻译器
///
/// 将 ActorSoul 的自然语言意图翻译为服务端格式化 Intent。
/// 不参与推理，只做数据映射。
///
/// 可选接收人魂的 CognitiveChain，提供认知上下文以增强指代消解。
pub struct IntentTranslator {
    llm_client: Arc<dyn crate::component::llm::LlmClient>,
}

impl IntentTranslator {
    pub fn new(llm_client: Arc<dyn crate::component::llm::LlmClient>) -> Self {
        Self { llm_client }
    }

    /// 多 Intent 翻译（Pipeline 模式）
    ///
    /// 将叙事意图拆分为多个结构化 Intent，支持 Pipeline 顺序执行。
    /// 用于天魂 translate_multi 场景：一个叙事意图可能包含多个步骤。
    pub async fn translate_multi(
        &self,
        narrative: &str,
        thought_log: &str,
        world_state: &WorldState,
        cognitive_chain: Option<&CognitiveChain>,
        max_intents: usize,
    ) -> Result<MultiTranslationResult> {
        let prompt = self.build_multi_prompt(
            narrative,
            thought_log,
            world_state,
            cognitive_chain,
            max_intents,
        );

        debug!("[天魂] 多Intent翻译: {}", narrative);

        let response: Vec<TranslationResponse> =
            tokio::time::timeout(std::time::Duration::from_secs(30), async {
                self.llm_client.complete_json(&prompt).await
            })
            .await
            .map_err(|_| anyhow::anyhow!("[天魂] 多Intent翻译超时"))??;

        let agent_id = world_state.agent_id.unwrap_or_default();
        let tick_id = world_state.tick_id;

        let mut intents: Vec<Intent> = response
            .into_iter()
            .take(max_intents)
            .map(|r| {
                let action_data = if r.action_data.is_null() {
                    None
                } else {
                    Some(r.action_data)
                };
                Intent::new(agent_id, tick_id, r.action_type.as_str(), action_data)
                    .with_thought(thought_log.to_string())
            })
            .collect();

        // 空结果 → idle
        if intents.is_empty() {
            intents.push(
                Intent::new(agent_id, tick_id, "idle", None).with_thought(thought_log.to_string()),
            );
        }

        // 路由说话意图
        let first = &intents[0];
        let action_type = first.action_type.as_str();
        let speech_intent = if matches!(action_type, "speak" | "whisper") {
            let speak = intents.remove(0);
            intents.insert(
                0,
                Intent::new(agent_id, tick_id, "idle", None).with_thought(thought_log.to_string()),
            );
            Some(speak)
        } else {
            None
        };

        Ok(MultiTranslationResult {
            intents,
            speech_intent,
            original_narrative: narrative.to_string(),
            original_thought_log: thought_log.to_string(),
        })
    }

    /// 构建多 Intent 翻译 prompt
    fn build_multi_prompt(
        &self,
        narrative: &str,
        thought_log: &str,
        world_state: &WorldState,
        cognitive_chain: Option<&CognitiveChain>,
        max_intents: usize,
    ) -> String {
        let inventory = if world_state.self_state.inventory.is_empty() {
            "空".to_string()
        } else {
            world_state
                .self_state
                .inventory
                .iter()
                .map(|i| format!("{} ({}): {} 个", i.item_id, i.name, i.quantity))
                .collect::<Vec<_>>()
                .join(", ")
        };

        let adjacent = if world_state.location.adjacent_nodes.is_empty() {
            "无".to_string()
        } else {
            world_state
                .location
                .adjacent_nodes
                .iter()
                .map(|n| format!("{} ({})", n.node_id, n.name))
                .collect::<Vec<_>>()
                .join(", ")
        };

        let entities = if world_state.entities.is_empty() {
            "无".to_string()
        } else {
            world_state
                .entities
                .iter()
                .map(|e| format!("{} [{}]", e.name, e.id))
                .collect::<Vec<_>>()
                .join(", ")
        };

        let nearby_items = if world_state.nearby_items.is_empty() {
            "无".to_string()
        } else {
            world_state
                .nearby_items
                .iter()
                .map(|i| format!("{} ({}): {} 个", i.item_id, i.name, i.quantity))
                .collect::<Vec<_>>()
                .join(", ")
        };

        let action_table = Self::build_action_table(&load_available_actions_from_file());
        let cognitive_context = Self::extract_cognitive_context(cognitive_chain);
        let cognitive_section = if cognitive_context.is_empty() {
            String::new()
        } else {
            format!("\n\n## Agent 认知轨迹\n{cognitive_context}")
        };

        format!(
            r#"你是意图翻译器。将角色的自然语言意图拆分为最多{max_intents}个按顺序执行的动作。

## 角色意图
{narrative}

## 思考过程
{thought_log}

## 当前精确数据
- 背包物品: {inventory}
- 可达位置: {adjacent}
- 附近的人: {entities}
- 地面物品: {nearby_items}

## 可用动作
{action_table}

## 规则
1. 拆分为按执行顺序排列的动作列表
2. 每个动作独立可执行，action_type 必须在可用动作表中
3. ID 必须使用精确英文 ID
4. 如果只有一个动作，输出单元素数组
5. 无法拆分时输出单个动作

## 输出格式
[{{"action_type": "动作名", "action_data": {{}}, "speech_content": ""}}]{cognitive_section}"#,
        )
    }

    fn build_action_table(actions: &[AvailableAction]) -> String {
        if actions.is_empty() {
            return "| idle | (无) | 休息 |".to_string();
        }

        let mut table = String::from(
            "| action | action_data 必填字段 | 说明 |\n|--------|---------------------|------|\n",
        );

        for action in actions {
            let fields = if action.required_fields.is_empty() {
                "(无)".to_string()
            } else {
                action
                    .required_fields
                    .iter()
                    .map(|f| format!("\"{}\"", f))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let desc = if action.description.is_empty() {
                &action.action
            } else {
                &action.description
            };
            table.push_str(&format!(
                "| {} | {{{}}} | {} |\n",
                action.action, fields, desc
            ));
        }

        table
    }

    /// 从人魂的 CognitiveChain 提取认知上下文
    ///
    /// 从各阶段的 metadata 中提取：
    /// - Perception.key_observations: 关键观察（包含感知到的人物、物品名称）
    /// - Motivation.primary_drive: 主要驱动力（揭示 agent 当前关注什么）
    /// - Decision.thought_process: 完整思考链（包含指代消解线索）
    ///
    /// 这些信息帮助天魂理解叙事中的指代词（如"他"、"她"、"那个"）指向谁/什么。
    fn extract_cognitive_context(chain: Option<&CognitiveChain>) -> String {
        let Some(chain) = chain else {
            return String::new();
        };

        let mut ctx = String::new();

        // 从 Perception stage 提取 key_observations
        if let Some(stage) = chain.get_stage(crate::soul::actor::CognitiveStage::Perception)
            && let Some(observations) = stage
                .metadata
                .get("key_observations")
                .and_then(|v| v.as_array())
        {
            let obs: Vec<&str> = observations
                .iter()
                .filter_map(|v| v.as_str())
                .take(5)
                .collect();
            if !obs.is_empty() {
                ctx.push_str("关键观察: ");
                ctx.push_str(&obs.join(", "));
                ctx.push('\n');
            }
        }

        // 从 Motivation stage 提取 primary_drive
        if let Some(stage) = chain.get_stage(crate::soul::actor::CognitiveStage::Motivation)
            && let Some(drive) = stage.metadata.get("primary_drive").and_then(|v| v.as_str())
        {
            ctx.push_str(&format!("当前驱动力: {}\n", drive));
        }

        // 从 Decision stage 提取完整 thought_process（包含人名、物品名引用）
        if let Some(stage) = chain.get_stage(crate::soul::actor::CognitiveStage::Decision)
            && let Some(thought) = stage
                .metadata
                .get("thought_process")
                .and_then(|v| v.as_str())
        {
            // 只取前 300 字，避免 prompt 过长
            let truncated = if thought.chars().count() > 300 {
                thought.chars().take(297).collect::<String>() + "..."
            } else {
                thought.to_string()
            };
            ctx.push_str(&format!("决策思考: {}\n", truncated));
        }

        if ctx.is_empty() { String::new() } else { ctx }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // extract_cognitive_context tests
    // ========================================================================

    #[test]
    fn test_extract_cognitive_context_none() {
        let result = IntentTranslator::extract_cognitive_context(None);
        assert_eq!(result, String::new());
    }

    #[test]
    fn test_extract_cognitive_context_with_observations() {
        use crate::soul::actor::{CognitiveChain, CognitiveStage, StageOutput};

        // 创建测试用 CognitiveChain
        let mut chain = CognitiveChain::from_persona(
            &crate::component::persona::DynamicPersona::new(
                uuid::Uuid::new_v4(),
                "测试角色",
                "你是一个测试角色",
            ),
            1,
        );

        // 添加 Perception stage
        let perception = StageOutput::with_metadata(
            CognitiveStage::Perception,
            "test perception".to_string(),
            serde_json::json!({
                "key_observations": ["张三在左边", "地上有个馒头"],
                "self_status": "饥饿",
                "environment": "客栈"
            }),
        );
        chain.add_stage(perception);

        // 添加 Motivation stage
        let motivation = StageOutput::with_metadata(
            CognitiveStage::Motivation,
            "test motivation".to_string(),
            serde_json::json!({
                "primary_drive": "获取食物",
                "drive_intensity": 8
            }),
        );
        chain.add_stage(motivation);

        // 测试提取
        let result = IntentTranslator::extract_cognitive_context(Some(&chain));
        assert!(result.contains("张三在左边"));
        assert!(result.contains("地上有个馒头"));
        assert!(result.contains("获取食物"));
    }
}
