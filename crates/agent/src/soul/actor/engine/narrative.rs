// ============================================================================
// 认知链叙事摘要入窗（push_summary_to_window）与情绪/叙事辅助
// ============================================================================

use super::*;

impl CognitiveEngine {
    // ========================================================================
    // 滑动上下文窗口
    // ========================================================================

    /// 将认知结果添加到滑动上下文窗口
    ///
    /// 由 lifecycle 在 ReflectorSoul 审查通过后调用（validated=true）。
    /// `validated=false` 用于记录被驳回的 intent（不参与行为重复检测）。
    pub fn push_summary_to_window(&self, chain: &CognitiveChain, intent: &Intent, validated: bool) {
        let full_decision = self.enrich_decision_full(intent);
        let decision = full_decision.clone();

        let perception = chain
            .get_stage(CognitiveStage::Perception)
            .map(|s| s.content.clone())
            .unwrap_or_default();

        let motivation = chain
            .get_stage(CognitiveStage::Motivation)
            .map(|s| s.content.clone())
            .unwrap_or_default();

        let summary = NarrativeSummary {
            tick_id: chain.tick_id,
            perception,
            motivation,
            decision,
            full_decision,
            outcome: "执行中".to_string(),
            validated,
        };

        self.push_summary(summary, validated);
    }

    /// 完整版 enrich_decision（不截断，用于语义去重比较）
    fn enrich_decision_full(&self, intent: &Intent) -> String {
        let action_type = intent.action_type.as_str();

        if let Some(data) = intent.action_data.as_ref()
            && let Some(content) = data.get("content").and_then(|v| v.as_str())
        {
            return format!("{}: \"{}\"", action_type, content);
        }
        action_type.to_string()
    }

    /// 添加摘要到滑动窗口
    pub fn push_summary(&self, summary: NarrativeSummary, validated: bool) {
        if let Ok(mut window) = self.summary_window.write() {
            window.push(summary, validated);
        }
    }

    /// 更新最近一条摘要的 outcome（Intent 执行结果写回）
    pub fn update_summary_outcome(&self, outcome: String) {
        if let Ok(mut window) = self.summary_window.write() {
            window.update_last_outcome(outcome);
        }
    }

    /// 记录行动结果到 Outcome Memory
    ///
    /// mem.record() 现在返回 Result，这里显式处理（warn + best-effort 继续）。
    pub fn record_outcome(&self, record: crate::component::memory::OutcomeRecord) {
        if let Some(ref mem) = self.outcome_memory
            && let Err(e) = mem.record(record)
        {
            tracing::warn!("record_outcome 失败，已 best-effort 忽略：{e:?}");
        }
    }

    /// 设置当前 tick 的对话上下文（由 lifecycle 每轮注入）
    pub fn set_dialogue_context(&self, context: String) {
        if let Ok(mut guard) = self.dialogue_context.write() {
            *guard = context;
        }
    }

    /// 获取滑动窗口上下文（用于 prompt 注入）
    pub fn get_summary_context(&self) -> String {
        if let Ok(window) = self.summary_window.read() {
            window.to_context()
        } else {
            String::new()
        }
    }

    /// 获取 Outcome Memory 上下文（公开接口，供 lifecycle snapshot 使用）
    pub fn get_outcome_context_public(&self) -> String {
        self.outcome_memory
            .as_ref()
            .map(|m| m.to_prompt_context())
            .unwrap_or_default()
    }

    /// 记忆叙事合成（人魂处理）
    ///
    /// 每 Tick 最多调用一次，将高重要性事件批量合成叙事。
    /// 失败时返回降级文本，不丢弃记忆。
    ///
    /// # Arguments
    /// * `events` - 高重要性事件（已按 importance_score 过滤）
    /// * `summary_context` - 前X回合行动摘要（来自 NarrativeSummaryWindow）
    /// * `outcome_context` - 行动结果学习（来自 OutcomeMemory）
    ///
    /// # Returns
    /// 叙事化文本（10-200字），或失败降级文本
    pub async fn synthesize_memory_narrative(
        &self,
        events: &[cyber_jianghu_protocol::WorldEvent],
        summary_context: &str,
        outcome_context: &str,
    ) -> String {
        // 1. 获取配置（从 prompt_templates.json 的 memory_narrative section）
        let prompt_cfg = self.prompt_template();
        let config = match prompt_cfg.get_memory_narrative_config() {
            Some(c) => c,
            None => {
                tracing::warn!("记忆叙事合成配置缺失，降级");
                return FALLBACK_NARRATIVE.to_string();
            }
        };

        // 2. 限制输入事件数
        let events_to_process = events.iter().take(config.max_events_per_tick);
        let events_list = events_to_process
            .map(|e| format!("- [{}] {}", e.event_type, e.description))
            .collect::<Vec<_>>()
            .join("\n");

        // 3. 构建 prompt
        let mut vars = std::collections::HashMap::new();
        vars.insert("events_list".to_string(), events_list);
        vars.insert(
            "summary_context".to_string(),
            if summary_context.is_empty() {
                "无近期行动记录".to_string()
            } else {
                summary_context.to_string()
            },
        );
        vars.insert(
            "outcome_context".to_string(),
            if outcome_context.is_empty() {
                "无行动结果学习".to_string()
            } else {
                outcome_context.to_string()
            },
        );
        vars.insert(
            "max_narrative_len".to_string(),
            config.max_narrative_len.to_string(),
        );

        let prompt = match self.prompt_template().render_memory_narrative(&vars) {
            Some(p) => p,
            None => {
                tracing::warn!("记忆叙事合成 prompt 渲染失败，降级");
                return FALLBACK_NARRATIVE.to_string();
            }
        };

        // 4. 调用 LLM
        let temperature = self.config.read().expect("rwlock poisoned").temperature;
        let chat_config = crate::component::llm::ChatExchangeConfig {
            model: self.llm_client.model_name(),
            temperature,
            max_tokens: None,
            enable_thinking: None,
        };
        let response: MemoryNarrativeResponse = match self
            .llm_client
            .complete_json_with_config_and_retry_extracted(&prompt, chat_config, 2)
            .await
        {
            Ok(extracted) => {
                if let Ok(mut rc) = self.last_reasoning_content.lock() {
                    *rc = extracted.reasoning_content;
                }
                extracted.value
            }
            Err(e) => {
                tracing::warn!("记忆叙事合成 LLM 调用失败: {}，降级", e);
                return FALLBACK_NARRATIVE.to_string();
            }
        };

        // 5. 验证输出
        let narrative = response.narrative.trim().to_string();
        if narrative.len() < config.min_narrative_len {
            tracing::warn!(
                "记忆叙事合成输出过短 ({} < {})，降级",
                narrative.len(),
                config.min_narrative_len
            );
            return FALLBACK_NARRATIVE.to_string();
        }

        narrative
    }

    /// 获取 Action Index（公开接口，供 API enrichment 使用）
    pub fn get_action_context(&self) -> (String, String) {
        let cache = self.prompt_cache.read().expect("rwlock poisoned");
        (cache.get_action_descriptions().to_string(), String::new())
    }

    /// 清空滑动窗口
    pub fn clear_summary_window(&self) {
        if let Ok(mut window) = self.summary_window.write() {
            window.clear();
        }
    }

    /// 获取最近 N 条同 action_type 的 validated 摘要的完整决策内容
    ///
    /// 用于 ReflectorSoul 语义去重：比较新 intent 与最近同类 intent 的语义相似度。
    pub fn get_recent_same_type_decisions(&self, action_type: &str, limit: usize) -> Vec<String> {
        self.summary_window
            .read()
            .map(|sw| sw.get_recent_same_type_decisions(action_type, limit))
            .unwrap_or_default()
    }
}
