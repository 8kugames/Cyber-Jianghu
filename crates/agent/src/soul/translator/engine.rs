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
use cyber_jianghu_protocol::AvailableAction;

/// 翻译结果
#[derive(Debug, Clone, Deserialize)]
pub struct TranslationResponse {
    /// 服务端动作类型（eat, move, idle, speak...）
    pub action_type: String,
    /// 动作参数（含精确 ID）
    #[serde(default)]
    pub action_data: serde_json::Value,
}

/// 天魂 — 意图翻译器
///
/// 将 ActorSoul 的自然语言意图翻译为服务端格式化 Intent。
/// 不参与推理，只做数据映射。
pub struct IntentTranslator {
    llm_client: Arc<dyn crate::component::llm::LlmClient>,
}

impl IntentTranslator {
    pub fn new(llm_client: Arc<dyn crate::component::llm::LlmClient>) -> Self {
        Self { llm_client }
    }

    /// 翻译叙事意图为结构化 Intent
    ///
    /// # Arguments
    /// * `narrative` - ActorSoul 的自然语言意图（如 "吃一个馒头来充饥"）
    /// * `thought_log` - ActorSoul 的思考过程
    /// * `world_state` - 当前世界状态（含背包物品 ID、可达位置 ID）
    pub async fn translate(
        &self,
        narrative: &str,
        thought_log: &str,
        world_state: &WorldState,
    ) -> Result<Intent> {
        let prompt = self.build_prompt(narrative, thought_log, world_state);

        debug!("[天魂] 翻译叙事意图: {}", narrative);

        let response: TranslationResponse = self.llm_client.complete_json(&prompt).await?;

        debug!(
            "[天魂] 翻译结果: action_type={}, action_data={:?}",
            response.action_type, response.action_data
        );

        let agent_id = world_state.agent_id.unwrap_or_default();
        let action_data = if response.action_data.is_null() {
            None
        } else {
            Some(response.action_data)
        };

        Ok(Intent::new(agent_id, world_state.tick_id, response.action_type.as_str(), action_data)
            .with_thought(thought_log.to_string()))
    }

    fn build_prompt(
        &self,
        narrative: &str,
        thought_log: &str,
        world_state: &WorldState,
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
            "无（无法移动）".to_string()
        } else {
            world_state
                .location
                .adjacent_nodes
                .iter()
                .map(|n| {
                    if n.travel_cost > 1 {
                        format!("{} ({}), 耗时{}tick", n.node_id, n.name, n.travel_cost)
                    } else {
                        format!("{} ({})", n.node_id, n.name)
                    }
                })
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

        format!(
            r#"你是意图翻译器。将角色的自然语言意图转换为服务端格式化 JSON。

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
1. action_type 必须是可用动作表中的动作名称
2. item_id 必须使用背包/地面物品中的英文 ID（如 mantou, water）
3. target_location 必须使用可达位置中的英文 ID（如 longmen_backyard）
4. target_agent_id 必须使用附近的人中的 agent_id
5. 没有对应动作时输出 idle
6. action_data 中的 key 必须与动作表的"必填字段"严格匹配

## 输出格式
{{"action_type": "动作名", "action_data": {{}}}}"#,
            narrative = narrative,
            thought_log = thought_log,
            inventory = inventory,
            adjacent = adjacent,
            entities = entities,
            nearby_items = nearby_items,
            action_table = action_table,
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
}
