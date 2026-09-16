// ============================================================================
// 认知推理主循环（think_direct 及 think 系列入口）
// ============================================================================

use super::*;

impl CognitiveEngine {
    // ========================================================================
    // 核心认知方法
    // ========================================================================

    /// 人魂直连 WorldState 认知流程
    ///
    /// 单次 LLM 调用，直接从 WorldState 生成结构化 Intent。
    /// Prompt 包含精确数据（item_id、node_id、entity UUID），
    /// LLM 直接输出 action_type + action_data（不再走天魂翻译）。
    ///
    /// 三区域分区调用：system（Immutable Prefix）→ semi-static → tick（Volatile）
    pub async fn think_direct(
        &self,
        world_state: &WorldState,
        memory_context: &str,
        validation_feedback: Option<&str>,
        soul_cycle_attempt: i32,
    ) -> Result<CognitiveChain> {
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
        info!("[{}-{}] 人魂直连认知流程开始...", agent_name, tick_id);

        let mut chain = CognitiveChain::from_persona(&persona, tick_id);

        let use_tool_calling = self.llm_client.supports_tool_calling();

        // FocusSummary + Critical preload
        let focus = self.current_focus_summary.read().await.clone();
        let critical_preload = if let Some(ref fs) = focus {
            self.preload_critical_data(fs).await
        } else {
            None
        };

        // === 三区域 Prompt 构建 ===

        // 0. 端侧"我眼中的江湖"：批量查询附近 entity 的关系认知（一次锁，避免 prompt 构造里逐个查）
        //    完全本地：每个 agent 只查自己的 relationship_store，不知道别人怎么看自己。
        //    尊重不对称：A 注入 A 对 B 的看法，B 注入 B 对 A 的看法，各自独立。
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

        // 1. tick message (volatile)
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

        // 2. 读取 semi-static 内容（由 rebuild_semi_static 维护）
        let semi_static = self
            .semi_static_message
            .read()
            .expect("rwlock poisoned")
            .clone();

        // 使用对话历史（长窗口）或单次调用
        let response: DirectCognitiveResponse = {
            let conv_data = self.conversation_history.as_ref().map(|history| {
                let h = history.lock().expect("lock poisoned");
                (
                    h.get_turns()
                        .iter()
                        .map(|t| ConversationTurn {
                            user: t.user.clone(),
                            assistant: t.assistant.clone(),
                            reasoning_content: t.reasoning_content.clone(),
                        })
                        .collect::<Vec<_>>(),
                    h.get_system_message().to_string(),
                    h.get_summary().map(|s| s.to_string()),
                )
            });
            // lock 已释放

            if use_tool_calling {
                // 地魂 tool-calling 路径（主路径）：LLM 可调用 skill_view / search_memory 等工具
                let memory_manager = self.memory_manager.read().expect("rwlock poisoned").clone();
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

                match conv_data {
                    Some((turns, system, summary)) => {
                        // tool-calling 模式下限制历史轮次（配置驱动，避免模式惯性）
                        let max_tool_turns = self.truncation("tool_calling_history_turns", 8);
                        let turns: Vec<_> = if turns.len() > max_tool_turns {
                            turns.into_iter().rev().take(max_tool_turns).rev().collect()
                        } else {
                            turns
                        };

                        // Tool-calling + 对话历史（正常部署路径）
                        self.llm_client
                            .complete_json_with_conversation_and_tools::<DirectCognitiveResponse>(
                                &system,
                                ConversationInput {
                                    semi_static: &semi_static,
                                    summary: summary.as_deref(),
                                    turns: &turns,
                                    current_prompt: &tick_msg,
                                },
                                &tools,
                                &executor,
                                self.llm_param("max_tool_rounds", 5),
                            )
                            .await?
                    }
                    None => {
                        // Tool-calling 无对话历史（降级）
                        let persona_for_prompt = {
                            let cache = self.prompt_cache.read().expect("rwlock poisoned");
                            cache.get_persona_simple().to_string()
                        };
                        self.llm_client
                            .complete_json_with_tools::<DirectCognitiveResponse>(
                                &persona_for_prompt,
                                &tick_msg,
                                &tools,
                                &executor,
                                self.llm_param("max_tool_rounds", 5),
                            )
                            .await?
                    }
                }
            } else {
                // 非 tool-calling 路径：非流式优先（默认），仅启用时尝试 streaming
                // 注意：streaming 不支持 tool-calling 组合
                match conv_data {
                    Some((turns, system, summary)) => {
                        if self.enable_streaming {
                            match self
                                .llm_client
                                .complete_json_streaming_with_conversation(
                                    &system,
                                    &semi_static,
                                    summary.as_deref(),
                                    &turns,
                                    &tick_msg,
                                )
                                .await
                            {
                                Ok(resp) => resp,
                                Err(e) => {
                                    tracing::warn!("流式调用失败，降级到非流式: {}", e);
                                    self.llm_client
                                        .complete_json_with_conversation(
                                            &system,
                                            &semi_static,
                                            summary.as_deref(),
                                            &turns,
                                            &tick_msg,
                                        )
                                        .await?
                                }
                            }
                        } else {
                            self.llm_client
                                .complete_json_with_conversation(
                                    &system,
                                    &semi_static,
                                    summary.as_deref(),
                                    &turns,
                                    &tick_msg,
                                )
                                .await?
                        }
                    }
                    None => {
                        let persona_for_prompt = {
                            let cache = self.prompt_cache.read().expect("rwlock poisoned");
                            cache.get_persona_simple().to_string()
                        };
                        let temperature = self.config.read().expect("rwlock poisoned").temperature;
                        if self.enable_streaming {
                            match self
                                .llm_client
                                .complete_json_streaming(&persona_for_prompt, &tick_msg)
                                .await
                            {
                                Ok(resp) => resp,
                                Err(e) => {
                                    tracing::warn!("流式调用失败，降级到非流式: {}", e);
                                    let chat_config = crate::component::llm::ChatExchangeConfig {
                                        model: self.llm_client.model_name(),
                                        temperature,
                                        max_tokens: None,
                                        enable_thinking: None,
                                    };
                                    let extracted = self
                                        .llm_client
                                        .complete_json_with_config_and_retry_extracted(
                                            &tick_msg,
                                            chat_config,
                                            2,
                                        )
                                        .await?;
                                    if let Ok(mut rc) = self.last_reasoning_content.lock() {
                                        *rc = extracted.reasoning_content;
                                    }
                                    extracted.value
                                }
                            }
                        } else {
                            let chat_config = crate::component::llm::ChatExchangeConfig {
                                model: self.llm_client.model_name(),
                                temperature,
                                max_tokens: None,
                                enable_thinking: None,
                            };
                            let extracted = self
                                .llm_client
                                .complete_json_with_config_and_retry_extracted(
                                    &tick_msg,
                                    chat_config,
                                    2,
                                )
                                .await?;
                            if let Ok(mut rc) = self.last_reasoning_content.lock() {
                                *rc = extracted.reasoning_content;
                            }
                            extracted.value
                        }
                    }
                }
            }
        };
        // 保存 reasoning_content 供 push_conversation_turn 使用
        // 仅当 LLM client 有 reasoning_content 时覆盖，避免 None 冲掉已保存值
        if let Ok(mut rc) = self.last_reasoning_content.lock()
            && let Some(rc_val) = self.llm_client.take_last_reasoning_content()
        {
            *rc = Some(rc_val);
        }
        // 提取 LLM 构造的情绪
        if let Some(ref emotion) = response.constructed_emotion
            && !emotion.label.is_empty()
            && let Ok(mut guard) = self.last_constructed_emotion.lock()
        {
            *guard = Some(emotion.clone());
        }
        let response_json = serde_json::to_string(&response)?;

        // 构建 CognitiveChain 的 4 个 stage（从统一响应中提取）
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
            serde_json::json!({
                "thought_process": response.thought_process,
            }),
        );
        chain.add_stage(planning);

        // 构建结构化 Intents（从 actions 数组，向后兼容旧格式）
        // LLM 必须精确输出 canonical action_type 和精确 ID，不做翻译
        let raw_actions = response.get_actions();
        let parsed_actions: Vec<DirectCognitiveAction> = raw_actions
            .iter()
            .map(|a| {
                Ok(DirectCognitiveAction {
                    action_type: a.action_type.clone(),
                    action_data: a.action_data.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let intents: Vec<Intent> = parsed_actions
            .iter()
            .map(|a| {
                Intent::new(
                    agent_id,
                    tick_id,
                    a.action_type.clone(),
                    a.action_data.clone(),
                )
                .with_thought(response.thought_process.clone())
            })
            .collect();

        let primary_action = &parsed_actions[0];
        let decision = super::super::stages::StageOutput::with_metadata(
            CognitiveStage::Decision,
            format!(
                "思考: {}\n决策: {} {:?}{}",
                response.thought_process,
                primary_action.action_type,
                primary_action.action_data,
                if parsed_actions.len() > 1 {
                    format!(" (+{} 后续)", parsed_actions.len() - 1)
                } else {
                    String::new()
                }
            ),
            serde_json::to_value(&response)?,
        );
        chain.add_stage(decision);
        chain.final_intent = intents[0].clone();
        chain.should_remember = response.should_remember;
        chain.memory_content = response.memory_content;

        thinking_log::log_llm(&agent_name, tick_id, "Direct", &tick_msg, &response_json);

        // 训练 trace（与 log_llm 并列，同源数据，不同输出：trace 给训练吃）
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
            response: response_json.clone(),
            prompt_tokens: None, // 架构限制：调用方拿不到 token（在 direct_client 层）
            completion_tokens: None,
            ok: true,
            wall_clock: chrono::Utc::now(),
        });

        chain.duration_ms = start_time.elapsed().as_millis() as u64;

        info!(
            "[{}-{}] 人魂直连认知完成，耗时 {}ms，决策: {} ({} 个 action)",
            agent_name,
            tick_id,
            chain.duration_ms,
            primary_action.action_type,
            parsed_actions.len()
        );

        thinking_log::log_thinking(&agent_name, tick_id, &chain.summarize());

        // 将 multi-intent 存入 chain metadata 供 lifecycle 读取
        chain.multi_intents = if intents.len() > 1 {
            Some(intents[1..].to_vec())
        } else {
            None
        };

        Ok(chain)
    }

    /// 旧式认知流程（不接收 WorldState，用于兼容旧回调路径）
    pub async fn think(&self, tick_id: i64, agent_id: Uuid) -> Result<CognitiveChain> {
        self.think_with_feedback(tick_id, agent_id, None).await
    }

    pub async fn think_with_feedback(
        &self,
        tick_id: i64,
        agent_id: Uuid,
        validation_feedback: Option<&str>,
    ) -> Result<CognitiveChain> {
        // Legacy 降级路径：attempt 信息不透传（trace::record 主要用 Direct 路径）
        self.think_with_memory_and_feedback(tick_id, agent_id, "", validation_feedback, 0)
            .await
    }

    /// 使用记忆上下文执行认知流程（旧式，用于兼容路径）
    pub async fn think_with_memory(
        &self,
        tick_id: i64,
        agent_id: Uuid,
        memory_context: &str,
    ) -> Result<CognitiveChain> {
        // Legacy 降级路径：attempt 信息不透传（trace::record 主要用 Direct 路径）
        self.think_with_memory_and_feedback(tick_id, agent_id, memory_context, None, 0)
            .await
    }

    /// 旧式核心认知流程（不接收 WorldState，降级路径用）
    pub(crate) async fn think_with_memory_and_feedback(
        &self,
        tick_id: i64,
        agent_id: Uuid,
        memory_context: &str,
        validation_feedback: Option<&str>,
        soul_cycle_attempt: i32,
    ) -> Result<CognitiveChain> {
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

        let start_time = std::time::Instant::now();
        info!("[{}-{}] 开始认知流程（旧式降级）...", agent_name, tick_id);

        let mut chain = CognitiveChain::from_persona(&persona, tick_id);

        // 降级：无 WorldState，用空占位。build_tick_message 会走 build_world_state_section 降级路径。
        let empty_ws = super::super::engine_prompts::empty_world_state();
        let tick_msg =
            self.build_tick_message(super::super::engine_prompts::TickMessageParams {
                world_state: &empty_ws,
                memory_context,
                validation_feedback,
                focus_summary: None,
                critical_preload: None,
                relationships: None, // 降级路径无 entity，无关系数据
            })?;

        let temperature = self.config.read().expect("rwlock poisoned").temperature;
        let chat_config = crate::component::llm::ChatExchangeConfig {
            model: self.llm_client.model_name(),
            temperature,
            max_tokens: None,
            enable_thinking: None,
        };
        let extracted = self
            .llm_client
            .complete_json_with_config_and_retry_extracted(&tick_msg, chat_config, 2)
            .await?;
        if let Ok(mut rc) = self.last_reasoning_content.lock() {
            *rc = extracted.reasoning_content;
        }
        let response: DirectCognitiveResponse = extracted.value;
        let response_json = serde_json::to_string(&response)?;

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

        // 旧式路径也支持多 action 格式
        // LLM 必须精确输出，不做翻译
        let actions = response.get_actions();
        let action_data = actions[0].action_data.clone();
        let intent = Intent::new(
            agent_id,
            tick_id,
            actions[0].action_type.clone(),
            action_data,
        )
        .with_thought(response.thought_process.clone());

        let decision = super::super::stages::StageOutput::with_metadata(
            CognitiveStage::Decision,
            format!(
                "思考: {}\n决策: {} {:?}",
                response.thought_process, actions[0].action_type, actions[0].action_data
            ),
            serde_json::to_value(&response)?,
        );
        chain.add_stage(decision);
        chain.final_intent = intent.clone();
        chain.should_remember = response.should_remember;
        chain.memory_content = response.memory_content;

        thinking_log::log_llm(&agent_name, tick_id, "Legacy", &tick_msg, &response_json);

        // 训练 trace（人魂 Legacy 路径）
        trace::record(trace::LlmTrace {
            trace_id: uuid::Uuid::new_v4().to_string(),
            agent_id,
            character_name: agent_name.clone(),
            tick_id,
            soul_stage: trace::SoulStage::Renhun,
            attempt: soul_cycle_attempt,
            provider: self.llm_client.provider_name(),
            model: self.llm_client.model_name(),
            persona_name: agent_name.clone(),
            persona_description: String::new(), // Legacy 降级路径无 persona 上下文
            user_prompt: tick_msg.clone(),
            response: response_json.clone(),
            prompt_tokens: None,
            completion_tokens: None,
            ok: true,
            wall_clock: chrono::Utc::now(),
        });

        chain.duration_ms = start_time.elapsed().as_millis() as u64;

        info!(
            "[{}-{}] 旧式认知完成，耗时 {}ms",
            agent_name, tick_id, chain.duration_ms
        );

        thinking_log::log_thinking(&agent_name, tick_id, &chain.summarize());

        Ok(chain)
    }
}
