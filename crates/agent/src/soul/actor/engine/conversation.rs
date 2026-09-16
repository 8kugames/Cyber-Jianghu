// ============================================================================
// 对话历史管理与上轮动作/驳回上下文
// ============================================================================

use super::*;

impl CognitiveEngine {
    pub fn set_conversation_history(&mut self, history: ConversationHistory) {
        info!(
            "对话历史已注入: {} 轮, tokens≈{}",
            history.turn_count(),
            history.estimated_tokens(),
        );
        self.conversation_history = Some(std::sync::Mutex::new(history));
        // 注入后同步 system message 和 semi-static
        let use_tool_calling = self.llm_client.supports_tool_calling();
        let system_msg = self.build_system_message(use_tool_calling);
        self.update_conversation_system_message(&system_msg);
        self.sync_semi_static_to_history();
    }

    /// 添加一轮对话到历史
    pub fn push_conversation_turn(
        &self,
        tick_id: i64,
        user: String,
        assistant: String,
        reasoning_content: Option<String>,
    ) {
        if let Some(ref history) = self.conversation_history
            && let Ok(mut h) = history.lock()
            && let Err(e) = h.push_turn(tick_id, user, assistant, reasoning_content)
        {
            tracing::warn!("对话历史写入失败: {}", e);
        }
    }

    /// 取回最近一次 LLM 调用的 reasoning_content
    pub fn take_last_reasoning_content(&self) -> Option<String> {
        self.last_reasoning_content
            .lock()
            .ok()
            .and_then(|mut g| g.take())
    }

    pub fn take_last_tool_call_log(&self) -> Option<Vec<cyber_jianghu_protocol::EarthToolCall>> {
        self.llm_client.take_last_tool_call_log()
    }

    /// 取回 LLM 构造的情绪（消费式，取后清空）
    pub fn take_constructed_emotion(&self) -> Option<ConstructedEmotion> {
        self.last_constructed_emotion
            .lock()
            .ok()
            .and_then(|mut g| g.take())
    }

    /// 读取当前 persona traits 引用（用于 CoreAffect 基线计算）
    pub fn persona_traits_snapshot(
        &self,
    ) -> std::collections::HashMap<String, crate::component::persona::Trait> {
        let guard = self.persona_ref.read().expect("rwlock poisoned");
        match guard.as_ref() {
            Some(arc) => arc.read(|p| p.traits.clone()),
            None => std::collections::HashMap::new(),
        }
    }

    /// 检查是否需要 summary 压缩
    pub fn conversation_needs_summary(&self) -> bool {
        if let Some(ref history) = self.conversation_history
            && let Ok(h) = history.lock()
        {
            return h.needs_summary();
        }
        false
    }

    /// 生成 summary prompt
    pub fn conversation_summary_prompt(&self) -> Option<String> {
        if let Some(ref history) = self.conversation_history
            && let Ok(h) = history.lock()
        {
            let prompt = h.generate_summary_prompt();
            if prompt.is_empty() {
                return None;
            }
            return Some(prompt);
        }
        None
    }

    /// summary 生成失败时降级为强制截断
    pub fn conversation_force_truncate(&self) {
        if let Some(ref history) = self.conversation_history
            && let Ok(mut h) = history.lock()
            && let Err(e) = h.force_truncate_to_recent()
        {
            tracing::error!("对话历史强制截断失败: {}", e);
        }
    }

    /// 执行 summary 压缩
    pub fn conversation_replace_with_summary(&self, summary: String) {
        if let Some(ref history) = self.conversation_history
            && let Ok(mut h) = history.lock()
            && let Err(e) = h.replace_with_summary(summary)
        {
            tracing::warn!("对话历史压缩失败: {}", e);
        }
    }

    /// 清空对话历史 (rebirth)
    pub fn clear_conversation_history(&self) {
        if let Some(ref history) = self.conversation_history
            && let Ok(mut h) = history.lock()
            && let Err(e) = h.clear()
        {
            tracing::warn!("对话历史清空失败: {}", e);
        }
    }

    /// 更新对话历史的上下文窗口上限（模型切换后调用）
    pub fn update_conversation_max_tokens(&self, max_tokens: usize) {
        if let Some(ref history) = self.conversation_history
            && let Ok(mut h) = history.lock()
        {
            h.update_max_tokens(max_tokens);
            tracing::info!("对话历史上下文窗口已更新: max_tokens={}", max_tokens);
        }
    }

    /// 更新对话历史的 system message (persona 变更时)
    pub fn update_conversation_system_message(&self, msg: &str) {
        if let Some(ref history) = self.conversation_history
            && let Ok(mut h) = history.lock()
        {
            h.update_system_message(msg);
        }
    }

    /// 更新动作索引（收到 game_rules_update 后调用）
    pub fn update_action_index(&self, actions: &[cyber_jianghu_protocol::AvailableAction]) {
        let descriptions = Self::build_action_index_pub(actions);
        let field_hints = String::new();
        {
            let mut cache = self.prompt_cache.write().expect("rwlock poisoned");
            cache.update_action_descriptions(descriptions, field_hints);
        }

        // 重建 semi-static 内容（action index 变更）
        self.rebuild_semi_static();
        // 同步到 ConversationHistory
        self.sync_semi_static_to_history();

        info!("动作列表已更新: {} 个动作", actions.len());
    }

    /// 获取 Outcome Memory 经验教训 prompt 段
    pub(in crate::soul::actor) fn get_outcome_context(&self) -> String {
        self.outcome_memory
            .as_ref()
            .map(|m| m.to_prompt_context())
            .unwrap_or_default()
    }

    /// 设置上轮行动执行结果摘要（由 lifecycle 在处理 ExecutionResult 后调用）
    pub fn set_last_tick_action_summary(&self, summary: String) {
        let mut guard = self
            .last_tick_action_summary
            .write()
            .expect("last_tick_action_summary lock not poisoned");
        *guard = summary;
    }

    /// 获取上轮行动执行结果摘要（供 build_tick_message 注入人魂推理上下文）
    pub(in crate::soul::actor) fn get_last_tick_action_summary(&self) -> String {
        self.last_tick_action_summary
            .read()
            .expect("last_tick_action_summary lock not poisoned")
            .clone()
    }

    /// 记录本 tick 天魂最终驳回（未执行的意图；由 soul_cycle 在循环收敛后写入）。
    /// 空串表示本 tick 无最终驳回，会清除旧记录
    pub fn set_last_tick_rejection(&self, lines: String, tick_id: i64) {
        let mut guard = self
            .last_tick_rejection
            .write()
            .expect("last_tick_rejection lock not poisoned");
        *guard = (lines, tick_id);
    }

    /// 读取上轮驳回记录（内容, 发生 tick）；无记录返回 (空串, 0)
    pub fn get_last_tick_rejection(&self) -> (String, i64) {
        self.last_tick_rejection
            .read()
            .expect("last_tick_rejection lock not poisoned")
            .clone()
    }
}
