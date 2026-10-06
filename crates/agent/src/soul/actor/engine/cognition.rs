// ============================================================================
// 认知-only 阶段（决策模型两段式管线 · 第一段）
// ============================================================================
//
// 决策模型启用时，人魂 LLM 只产出认知（self_status / environment /
// key_observations / primary_drive / drive_intensity / thought_process /
// should_remember / memory_content / constructed_emotion），不再写 actions
// JSON——动作选择与实体绑定交给决策模型（component::decision_model）。
//
// 状态文本拼装（供决策模型读取）与训练数据 prepare_decision_sft.py 的
// "system persona + tick user prompt + 认知摘要块" 完全一致：
//   state = system_message + "\n\n" + tick_message + cognition_block
// 本模块产出上述三段；决策问题构造与门控在 runtime/decision.rs。

use super::*;

/// 认知-only 输出格式（替代模板 output_format 段；去掉 actions 要求，保留认知字段）
const COGNITION_OUTPUT_FORMAT: &str = r#"
## 输出格式
直接输出以下 JSON，不要在 JSON 前输出任何推理或思考文本。你的整个输出必须是且仅是一个 JSON 对象。
本阶段只输出认知，不输出 actions 数组，也不选择动作——动作由系统在此之后的结构化决策阶段完成。你的 thought_process 描述行动思路即可。
{
  "self_status": "用一句话描述你此刻的身体和精神状况，例如：饥饿难耐、内力充沛、精神恍惚",
  "environment": "用一句话描述你当前所在地点的景象，例如：厨房里弥漫着烟火气、大堂中三两客人低声交谈",
  "key_observations": ["你注意到的关键事物或人物，例如：桌上有半壶酒、角落里坐着一位陌生人"],
  "primary_drive": "此刻最强烈的内在冲动，例如：饥饿驱使觅食、好奇心驱使接近陌生人",
  "drive_intensity": 5,
  "thought_process": "基于上述感知和驱动，推演你接下来的行动思路 (500字以内)",
  "should_remember": false,
  "memory_content": "",
  "constructed_emotion": {
    "label": "一个中文情绪词（如：愤怒、感激、忐忑、释然、哀伤、欣喜）",
    "reasoning": "为什么产生这种情绪（1-2句，结合体感信号）",
    "intensity": 0.5
  }
}

### 记忆判定：
如果你认为当前情境中的某些事件值得长期记忆（如：与其他侠士的重要互动、目标达成、重大发现），设置：
- "should_remember": true
- "memory_content": 用简洁的第一人称描述要记忆的内容（必须是真实发生的事件，不允许虚构）

否则设置 "should_remember": false。

### 情绪构造：
根据你的体感信号（愉悦度、激动程度），构造你此刻的具体情绪。
- label：一个精确的中文情绪词，反映体感+语境的综合判断（不是简单重复体感标签）
- reasoning：为什么产生这种情绪（结合体感和当前处境）
- intensity：情绪强度 0.0-1.0（体感越强烈，intensity 越高）
"#;

/// 认知-only 阶段产物（决策模型管线的输入）
pub(crate) struct CognitionOutput {
    /// system 消息（决策状态文本第一段）
    pub system_message: String,
    /// tick 消息（决策状态文本第二段）
    pub tick_message: String,
    /// 认知摘要块（格式与训练分布逐字一致；拼装在 tick_message 之后）
    pub cognition_block: String,
    /// 已填好 感知/动机/规划 三阶段的认知链（决策阶段由调用方补）
    pub chain: CognitiveChain,
    /// 记忆判定（透传自 LLM）
    pub should_remember: Option<bool>,
    pub memory_content: Option<String>,
    /// 认知 LLM 调用耗时
    pub duration_ms: u64,
}

impl CognitiveEngine {
    /// 认知-only 决策：单次 LLM 调用产出认知摘要，不输出动作
    ///
    /// 与 think_direct 共用 tick message 构建、对话历史与地魂 tool loop；
    /// 差异仅在 system message 的 output_format 段（COGNITION_OUTPUT_FORMAT）。
    pub(crate) async fn think_cognition_only(
        &self,
        world_state: &WorldState,
        memory_context: &str,
        validation_feedback: Option<&str>,
        soul_cycle_attempt: i32,
    ) -> Result<CognitionOutput> {
        let agent_name = {
            let cfg = self.config.read().expect("rwlock poisoned");
            cfg.agent_name.clone()
        };
        let persona = self
            .persona_ref
            .read()
            .expect("rwlock poisoned")
            .clone()
            .expect("persona_ref not set — call set_persona_ref after build")
            .read(|p| p.clone());
        let tick_id = world_state.tick_id;
        let agent_id = world_state.agent_id.unwrap_or_default();

        let start_time = std::time::Instant::now();
        info!(
            "[{}-{}] 人魂认知-only 阶段开始（决策模型管线）",
            agent_name, tick_id
        );

        let mut chain = CognitiveChain::from_persona(&persona, tick_id);

        let use_tool_calling = self.llm_client.supports_tool_calling();

        // FocusSummary + Critical preload（与 think_direct 同源）
        let focus = self.current_focus_summary.read().await.clone();
        let critical_preload = if let Some(ref fs) = focus {
            self.preload_critical_data(fs).await
        } else {
            None
        };

        // 端侧关系认知（与 think_direct 同源）
        let relationships_map: std::collections::HashMap<
            uuid::Uuid,
            crate::component::social::RelationshipMemory,
        > = {
            let guard = self.relationship_store.read().expect("rwlock poisoned");
            match guard.as_ref() {
                Some(store) => world_state
                    .entities
                    .iter()
                    .filter_map(|e| {
                        store
                            .get_relationship(e.id)
                            .ok()
                            .flatten()
                            .map(|r| (e.id, r))
                    })
                    .collect(),
                None => std::collections::HashMap::new(),
            }
        };

        // tick message（volatile，与 think_direct 完全一致）
        let tick_msg =
            self.build_tick_message(super::super::engine_prompts::TickMessageParams {
                world_state,
                memory_context,
                validation_feedback,
                focus_summary: focus.as_ref(),
                critical_preload: critical_preload.as_deref(),
                relationships: if relationships_map.is_empty() {
                    None
                } else {
                    Some(&relationships_map)
                },
            })?;

        // cognition-only system message（output_format 覆盖）
        let system_msg =
            self.build_system_message_inner(use_tool_calling, Some(COGNITION_OUTPUT_FORMAT));

        // semi-static 内容提前快照（避免跨 await 持有 std 锁）
        let semi_static = self
            .semi_static_message
            .read()
            .expect("rwlock poisoned")
            .clone();

        let response: DirectCognitiveResponse = crate::component::llm::scenario::with_scenario(
            crate::component::llm::scenario::THINK,
            async {
                let conv_data = self.conversation_history.as_ref().map(|history| {
                    let h = history.lock().expect("lock poisoned");
                    (
                        h.get_turns()
                            .iter()
                            .map(|t| crate::component::llm::ConversationTurn {
                                user: t.user.clone(),
                                assistant: t.assistant.clone(),
                                reasoning_content: t.reasoning_content.clone(),
                            })
                            .collect::<Vec<_>>(),
                        h.get_system_message().to_string(),
                        h.get_summary().map(|s| s.to_string()),
                    )
                });
                let response: DirectCognitiveResponse = if use_tool_calling {
                    // 地魂 tool-calling 路径（主路径）
                    let memory_manager =
                        self.memory_manager.read().expect("rwlock poisoned").clone();
                    let recipe_details = world_state.self_state.recipe_details.clone();
                    let world_state_store = self
                        .world_state_store
                        .read()
                        .expect("rwlock poisoned")
                        .clone();
                    let available_actions = self
                        .available_actions
                        .read()
                        .expect("rwlock poisoned")
                        .clone();
                    let rule_cache = self.rule_cache.read().expect("rwlock poisoned").clone();
                    let prompt_template_for_tool = self.prompt_template();
                    let executor = super::super::super::earth::EarthToolExecutor::from_context(
                        super::super::super::earth::EarthToolContext {
                            skill_cache: self.skill_cache.read().expect("rwlock poisoned").clone(),
                            memory_manager,
                            relationship_store: self
                                .relationship_store
                                .read()
                                .expect("rwlock poisoned")
                                .clone(),
                            recipe_details,
                            world_state_store,
                            available_actions,
                            rule_cache,
                            prompt_template: Some(std::sync::Arc::new(prompt_template_for_tool)),
                        },
                    );
                    let tools = executor.tool_definitions();
                    let max_tool_rounds = self.llm_param("max_tool_rounds", 5);
                    match conv_data {
                        Some((turns, _system, summary)) => {
                            let max_tool_turns = self.truncation("tool_calling_history_turns", 8);
                            let turns: Vec<_> = if turns.len() > max_tool_turns {
                                turns.into_iter().rev().take(max_tool_turns).rev().collect()
                            } else {
                                turns
                            };
                            self.llm_client
                                .complete_json_with_conversation_and_tools::<
                                    DirectCognitiveResponse,
                                >(
                                    &system_msg,
                                    crate::component::llm::ConversationInput {
                                        semi_static: &semi_static,
                                        summary: summary.as_deref(),
                                        turns: &turns,
                                        current_prompt: &tick_msg,
                                    },
                                    &tools,
                                    &executor,
                                    max_tool_rounds,
                                )
                                .await?
                        }
                        None => {
                            self.llm_client
                                .complete_json_with_tools::<DirectCognitiveResponse>(
                                    &system_msg,
                                    &tick_msg,
                                    &tools,
                                    &executor,
                                    max_tool_rounds,
                                )
                                .await?
                        }
                    }
                } else {
                    // 非 tool-calling：优先对话历史路径
                    match conv_data {
                        Some((turns, _system, summary)) => {
                            self.llm_client
                                .complete_json_with_conversation(
                                    &system_msg,
                                    &semi_static,
                                    summary.as_deref(),
                                    &turns,
                                    &tick_msg,
                                )
                                .await?
                        }
                        None => {
                            let temperature =
                                self.config.read().expect("rwlock poisoned").temperature;
                            let chat_config = crate::component::llm::ChatExchangeConfig {
                                model: self.llm_client.model_name(),
                                temperature,
                                max_tokens: None,
                                enable_thinking: None,
                            };
                            self.llm_client
                                .complete_json_with_system_and_retry_extracted::<
                                    DirectCognitiveResponse,
                                >(&system_msg, &tick_msg, chat_config, 2)
                                .await?
                                .value
                        }
                    }
                };
                Ok::<DirectCognitiveResponse, anyhow::Error>(response)
            },
        )
        .await?;

        // reasoning_content 保存（对话历史回传口径与 think_direct 一致）
        if let Ok(mut rc) = self.last_reasoning_content.lock()
            && let Some(rc_val) = self.llm_client.take_last_reasoning_content()
        {
            *rc = Some(rc_val);
        }
        // LLM 构造的情绪缓存（供 lifecycle 回写 persona）
        if let Some(ref emotion) = response.constructed_emotion
            && !emotion.label.is_empty()
            && let Ok(mut guard) = self.last_constructed_emotion.lock()
        {
            *guard = Some(emotion.clone());
        }

        let cognition_json = serde_json::to_string(&response)?;
        let cognition_block = crate::component::decision_model::cognition_block(
            &json_value_to_string(&response.self_status),
            &json_value_to_string(&response.environment),
            &response.key_observations,
            &response.primary_drive,
            response.drive_intensity,
            &response.thought_process,
            response
                .constructed_emotion
                .as_ref()
                .map(|e| (e.label.as_str(), e.intensity)),
        );

        // 三阶段（决策阶段由调用方依决策模型结果补全）
        let perception = super::super::stages::StageOutput::with_metadata(
            CognitiveStage::Perception,
            format!(
                "自身状态: {}\n环境: {}\n关键观察: {}",
                response.self_status,
                response.environment,
                response.key_observations.join(", ")
            ),
            serde_json::json!({
                "self_status": response.self_status,
                "environment": response.environment,
                "key_observations": response.key_observations,
            }),
        );
        chain.add_stage(perception);

        let motivation = super::super::stages::StageOutput::with_metadata(
            CognitiveStage::Motivation,
            format!(
                "主要驱动力: {} (强度: {}/10)",
                response.primary_drive, response.drive_intensity
            ),
            serde_json::json!({
                "primary_drive": response.primary_drive,
                "drive_intensity": response.drive_intensity,
            }),
        );
        chain.add_stage(motivation);

        let planning = super::super::stages::StageOutput::with_metadata(
            CognitiveStage::Planning,
            response
                .thought_process
                .chars()
                .take(self.truncation("planning_description", 100))
                .collect(),
            serde_json::json!({ "thought_process": response.thought_process }),
        );
        chain.add_stage(planning);

        thinking_log::log_llm(
            &agent_name,
            tick_id,
            "CognitionOnly",
            &tick_msg,
            &cognition_json,
        );

        // 训练 trace（认知-only 输出无 actions，下游 SFT 导出按既有口径自然跳过）
        trace::record(trace::LlmTrace {
            trace_id: uuid::Uuid::new_v4().to_string(),
            agent_id,
            character_name: agent_name.clone(),
            tick_id,
            soul_stage: trace::SoulStage::Renhun,
            attempt: soul_cycle_attempt,
            provider: self.llm_client.provider_name(),
            model: self.llm_client.model_name(),
            persona_name: persona.name.clone(),
            persona_description: persona.base_description.clone(),
            user_prompt: tick_msg.clone(),
            response: cognition_json.clone(),
            prompt_tokens: None,
            completion_tokens: None,
            ok: true,
            wall_clock: chrono::Utc::now(),
        });

        let duration_ms = start_time.elapsed().as_millis() as u64;
        info!(
            "[{}-{}] 人魂认知-only 完成，耗时 {}ms",
            agent_name, tick_id, duration_ms
        );

        Ok(CognitionOutput {
            system_message: system_msg,
            tick_message: tick_msg,
            cognition_block,
            chain,
            should_remember: response.should_remember,
            memory_content: response.memory_content,
            duration_ms,
        })
    }
}

/// 自我状态/环境字段 → 文本（Value::String 原样；其余 JSON 序列化——
/// 正常路径 LLM 输出一句话字符串，对象形态是罕见的偏离输出）
fn json_value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    /// 认知-only 输出格式不含 actions 要求（两段式边界）
    #[test]
    fn cognition_output_format_has_no_actions() {
        let fmt = super::COGNITION_OUTPUT_FORMAT;
        assert!(
            !fmt.contains("\"actions\""),
            "认知-only 格式不得要求 actions"
        );
        assert!(fmt.contains("self_status"));
        assert!(fmt.contains("key_observations"));
        assert!(fmt.contains("constructed_emotion"));
        assert!(fmt.contains("不输出 actions"), "须明示动作由决策阶段完成");
    }
}
