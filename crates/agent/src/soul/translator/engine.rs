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

/// 天魂翻译结果
#[derive(Debug)]
pub struct TranslationResult {
    /// 主行动 Intent（走正常 intent 配额）
    pub intent: Intent,
    /// 说话 Intent（speak/whisper，与主行动分离，由 lifecycle 决定发送方式）
    /// - 纯 speak/whisper: 整个 intent 搬到此处，主 intent 变 idle
    /// - 混合说话+行动: 提取的说话内容包装为 speak intent
    /// - shout: 不拆分，留在主 intent
    /// - 无说话: None
    pub speech_intent: Option<Intent>,
    /// 原始叙事文本（用于记录）
    pub original_narrative: String,
    /// 原始思考日志（用于记录）
    pub original_thought_log: String,
    /// 翻译是否成功
    pub success: bool,
    /// 翻译错误信息（失败时）
    pub error: Option<String>,
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

    /// 翻译叙事意图为结构化 Intent + 即时说话
    ///
    /// # Arguments
    /// * `narrative` - ActorSoul 的自然语言意图（如 "一边说'你好'，一边吃馒头"）
    /// * `thought_log` - ActorSoul 的思考过程
    /// * `world_state` - 当前世界状态（含背包物品 ID、可达位置 ID）
    ///
    /// 内置 30 秒超时保护，避免单次 LLM 调用吃掉整个 tick deadline。
    pub async fn translate(
        &self,
        narrative: &str,
        thought_log: &str,
        world_state: &WorldState,
    ) -> Result<TranslationResult> {
        let prompt = self.build_prompt(narrative, thought_log, world_state);

        debug!("[天魂] 翻译叙事意图: {}", narrative);

        // 30 秒超时保护，外层 lifecycle deadline 也会截断
        let translate_future = self
            .llm_client
            .complete_json::<TranslationResponse>(&prompt);
        let response = tokio::time::timeout(std::time::Duration::from_secs(30), translate_future)
            .await
            .map_err(|_| anyhow::anyhow!("[天魂] 翻译超时（30秒），降级为 idle"))??;

        debug!(
            "[天魂] 翻译结果: action_type={}, action_data={:?}, speech_content={:?}",
            response.action_type, response.action_data, response.speech_content
        );

        let agent_id = world_state.agent_id.unwrap_or_default();
        let action_data = if response.action_data.is_null() {
            None
        } else {
            Some(response.action_data)
        };

        let intent = Intent::new(
            agent_id,
            world_state.tick_id,
            response.action_type.as_str(),
            action_data,
        )
        .with_thought(thought_log.to_string());

        // 决定即时 intent 和主 intent 的分配
        let (main_intent, speech_intent) =
            self.route_intents(intent, response.speech_content.as_deref(), narrative);

        debug!(
            "[天魂] 路由结果: main={}/{:?}, speech={:?}",
            main_intent.action_type,
            main_intent.action_data,
            speech_intent
                .as_ref()
                .map(|i| format!("{}:{:?}", i.action_type, i.action_data))
        );

        Ok(TranslationResult {
            intent: main_intent,
            speech_intent,
            original_narrative: narrative.to_string(),
            original_thought_log: thought_log.to_string(),
            success: true,
            error: None,
        })
    }

    /// 路由：决定哪个 intent 走即时通道，哪个走主配额
    ///
    /// 规则：
    /// - 纯 speak/whisper: 整个 intent → 即时，主 intent 变 idle
    /// - shout: 留在主 intent（大喊占配额）
    /// - 混合（说话+行动）: 提取说话 → 即时 speak intent，行动留在主 intent
    /// - 无说话: immediate = None
    fn route_intents(
        &self,
        intent: Intent,
        llm_speech: Option<&str>,
        narrative: &str,
    ) -> (Intent, Option<Intent>) {
        let action_type = intent.action_type.as_str();

        // 纯 speak/whisper → 整个走即时通道
        if matches!(action_type, "speak" | "whisper") {
            debug!("[天魂] 纯 {} → 即时通道", action_type);
            let idle_intent = Intent::new(intent.agent_id, intent.tick_id, "idle", None)
                .with_thought(intent.thought_log.clone().unwrap_or_default());
            return (idle_intent, Some(intent));
        }

        // shout 保留在主 intent（大喊占配额）
        if action_type == "shout" {
            return (intent, None);
        }

        // 混合场景：提取说话内容
        let speech = self.extract_speech(llm_speech, narrative);
        if let Some(content) = speech {
            let speak_intent = Intent::new(
                intent.agent_id,
                intent.tick_id,
                "speak",
                Some(serde_json::json!({"content": content})),
            );
            return (intent, Some(speak_intent));
        }

        (intent, None)
    }

    /// 从叙事中提取说话内容
    ///
    /// 策略：
    /// 1. 优先使用 LLM 返回的 speech_content
    /// 2. Fallback: 正则从 narrative 中提取引号内容
    fn extract_speech(&self, llm_speech: Option<&str>, narrative: &str) -> Option<String> {
        // 优先使用 LLM 提取的结果
        if let Some(speech) = llm_speech {
            let trimmed = speech.trim();
            if !trimmed.is_empty() {
                debug!("[天魂] LLM 提取说话内容: {}", trimmed);
                return Some(trimmed.to_string());
            }
        }

        // Fallback: 正则从叙事中提取（一边说'xxx' / 说着'xxx'）
        if let Ok(re) = regex::Regex::new(r#"说[着了]?['"「]([^'"」]+)['"」]"#)
            && let Some(caps) = re.captures(narrative)
            && let Some(m) = caps.get(1)
        {
            let speech = m.as_str().to_string();
            if !speech.is_empty() {
                debug!("[天魂] 正则 fallback 提取说话内容: {}", speech);
                return Some(speech);
            }
        }

        None
    }

    fn build_prompt(&self, narrative: &str, thought_log: &str, world_state: &WorldState) -> String {
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
7. 如果叙事中包含说话内容（如"一边说'xxx'，一边做Y"），将说话内容提取到 speech_content 字段，action_type 设为说话时的物理动作
8. 如果叙事纯说话无动作，action_type 设为 "speak"，content 放入 action_data.content，speech_content 留空

## 关键区分：eat/drink vs pickup
- **eat**（吃）：消耗背包中的食物（如馒头、肉干）→ item_id 必须在背包物品中
- **drink**（喝）：消耗背包中的饮品（如水壶、茶）→ item_id 必须在背包物品中
- **pickup**（捡）：从地面拾取物品到背包 → item_id 必须在地面物品中
- 角色"想吃东西/喝水/充饥/解渴" → 优先 eat/drink（物品在背包时）
- 角色"想捡起地上的东西" → pickup
- 背包有水却说"喝水"→ drink（不是 pickup！）
- 背包有馒头却说"吃馒头"→ eat（不是 pickup！）

## 输出格式
{{"action_type": "动作名", "action_data": {{}}, "speech_content": "说话内容或空字符串"}}"#,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::llm::MockLlmClient;
    use uuid::Uuid;

    fn make_translator() -> IntentTranslator {
        IntentTranslator::new(std::sync::Arc::new(MockLlmClient::with_response("")))
    }

    fn make_intent(action_type: &str, action_data: Option<serde_json::Value>) -> Intent {
        Intent::new(Uuid::new_v4(), 42, action_type, action_data)
            .with_thought("test thought".to_string())
    }

    // ========================================================================
    // route_intents tests
    // ========================================================================

    #[test]
    fn test_route_pure_speak_to_immediate() {
        let translator = make_translator();
        let intent = make_intent("speak", Some(serde_json::json!({"content": "你好"})));
        let (main, speech) = translator.route_intents(intent, None, "对大家说你好");

        assert_eq!(main.action_type.as_str(), "idle");
        assert!(main.action_data.is_none());
        assert!(speech.is_some());
        let sp = speech.unwrap();
        assert_eq!(sp.action_type.as_str(), "speak");
        assert_eq!(sp.action_data.unwrap()["content"], "你好");
    }

    #[test]
    fn test_route_pure_whisper_to_immediate() {
        let translator = make_translator();
        let intent = make_intent(
            "whisper",
            Some(serde_json::json!({"content": "秘密", "target_agent_id": "abc"})),
        );
        let (main, speech) = translator.route_intents(intent, None, "悄悄说秘密");

        assert_eq!(main.action_type.as_str(), "idle");
        assert!(speech.is_some());
        let sp = speech.unwrap();
        assert_eq!(sp.action_type.as_str(), "whisper");
    }

    #[test]
    fn test_route_shout_stays_main() {
        let translator = make_translator();
        let intent = make_intent("shout", Some(serde_json::json!({"content": "救命"})));
        let (main, speech) = translator.route_intents(intent, None, "大喊救命");

        assert_eq!(main.action_type.as_str(), "shout");
        assert!(speech.is_none());
    }

    #[test]
    fn test_route_mixed_with_llm_speech() {
        let translator = make_translator();
        let intent = make_intent("eat", Some(serde_json::json!({"item_id": "mantou"})));
        let (main, speech) = translator.route_intents(intent, Some("你好"), "一边说你好一边吃馒头");

        // main keeps eat
        assert_eq!(main.action_type.as_str(), "eat");
        assert_eq!(main.action_data.unwrap()["item_id"], "mantou");
        // speech extracted
        assert!(speech.is_some());
        let sp = speech.unwrap();
        assert_eq!(sp.action_type.as_str(), "speak");
        assert_eq!(sp.action_data.unwrap()["content"], "你好");
    }

    #[test]
    fn test_route_mixed_with_regex_speech() {
        let translator = make_translator();
        let intent = make_intent("eat", Some(serde_json::json!({"item_id": "mantou"})));
        let (main, speech) = translator.route_intents(intent, None, "一边说'你好'一边吃馒头");

        assert_eq!(main.action_type.as_str(), "eat");
        assert!(speech.is_some());
        let sp = speech.unwrap();
        assert_eq!(sp.action_type.as_str(), "speak");
        assert_eq!(sp.action_data.unwrap()["content"], "你好");
    }

    #[test]
    fn test_route_no_speech() {
        let translator = make_translator();
        let intent = make_intent("idle", None);
        let (main, speech) = translator.route_intents(intent, None, "静静坐着");

        assert_eq!(main.action_type.as_str(), "idle");
        assert!(speech.is_none());
    }

    // ========================================================================
    // extract_speech tests
    // ========================================================================

    #[test]
    fn test_extract_speech_llm_provided() {
        let translator = make_translator();
        let result = translator.extract_speech(Some("你好世界"), "一边说'xxx'一边走");
        assert_eq!(result.as_deref(), Some("你好世界"));
    }

    #[test]
    fn test_extract_speech_llm_empty_falls_back_to_regex() {
        let translator = make_translator();
        let result = translator.extract_speech(Some(""), "一边说'你好'一边走");
        assert_eq!(result.as_deref(), Some("你好"));
    }

    #[test]
    fn test_extract_speech_regex_single_quotes() {
        let translator = make_translator();
        let result = translator.extract_speech(None, "一边说'你好'一边走");
        assert_eq!(result.as_deref(), Some("你好"));
    }

    #[test]
    fn test_extract_speech_regex_double_quotes() {
        let translator = make_translator();
        let result = translator.extract_speech(None, r#"说着"小心脚下""#);
        assert_eq!(result.as_deref(), Some("小心脚下"));
    }

    #[test]
    fn test_extract_speech_regex_corner_brackets() {
        let translator = make_translator();
        let result = translator.extract_speech(None, "说「天机不可泄露」");
        assert_eq!(result.as_deref(), Some("天机不可泄露"));
    }

    #[test]
    fn test_extract_speech_regex_with_zhe() {
        let translator = make_translator();
        let result = translator.extract_speech(None, "说着'出发吧'然后走了");
        assert_eq!(result.as_deref(), Some("出发吧"));
    }

    #[test]
    fn test_extract_speech_empty() {
        let translator = make_translator();
        let result = translator.extract_speech(None, "吃馒头充饥");
        assert!(result.is_none());
    }
}
