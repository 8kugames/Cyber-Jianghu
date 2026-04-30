// ============================================================================
// 社交事件处理
// ============================================================================
//
// 处理 WorldEvent 中的社交事件，通过 LLM 评估好感度变化并更新关系存储。
// 异步非阻塞：spawn 独立任务处理 LLM 调用。
// ============================================================================

use crate::component::llm::LlmClientExt;

impl super::Agent {
    /// 处理社交事件并更新关系存储
    ///
    /// 从 WorldEvent 列表中过滤 SocialInteraction 类型事件，
    /// 使用 LLM 评估每件事件的好感度变化（-10 到 +10），
    /// 异步更新关系存储。
    pub fn process_social_events(
        &self,
        events: &[crate::models::WorldEvent],
        entities: &[crate::models::Entity],
    ) {
        let Some(ref store) = self.relationship_store else {
            return;
        };

        // 收集所有社交事件（物品转移 + 公开说话 + 密语）
        let social_events: Vec<crate::models::WorldEvent> = events
            .iter()
            .filter(|e| {
                matches!(
                    e.event_type,
                    crate::models::WorldEventType::SocialInteraction
                        | crate::models::WorldEventType::PublicMessage
                        | crate::models::WorldEventType::PrivateDialogue
                )
            })
            .cloned()
            .collect();

        if social_events.is_empty() {
            return;
        }

        // 构建名称查找表（UUID → 名称）
        let name_map: std::collections::HashMap<String, String> = entities
            .iter()
            .map(|e| (e.id.to_string(), e.name.clone()))
            .collect();

        let container = self.actor_llm_container.clone();
        let store = store.clone();

        // 非阻塞：spawn 独立任务处理 LLM 调用和关系更新
        tokio::spawn(async move {
            // 构建 LLM 评估 prompt
            let event_descriptions: Vec<String> = social_events
                .iter()
                .enumerate()
                .map(|(i, e)| format!("{}. {}", i + 1, e.description))
                .collect();

            let prompt = format!(
                r#"你是一个武侠世界角色的内心评估器。根据以下社交事件，评估每件事对你对这个人的好感度变化。

返回 JSON 数组，每个元素包含 {{"index": 事件编号, "delta": 好感度变化(-10到+10的整数)}}
- 正数表示好感增加（如对方帮助、送礼）
- 负数表示好感降低（如被偷窃、被骗）
- 0 表示中性事件
- 考虑事件的具体内容和上下文

事件列表:
{}

只输出 JSON 数组，不要其他文字。"#,
                event_descriptions.join("\n")
            );

            // 调用 LLM 评估（无 LLM 容器则跳过，不写入 delta=0 的无意义记录）
            let Some(ref container) = container else {
                return;
            };

            let deltas: std::collections::HashMap<usize, i32> = match container
                .read()
                .await
                .complete_json::<Vec<serde_json::Value>>(&prompt)
                .await
            {
                Ok(results) => results
                    .into_iter()
                    .filter_map(|v| {
                        let idx = v.get("index")?.as_u64()? as usize;
                        let delta = v.get("delta")?.as_i64()? as i32;
                        Some((idx, delta.clamp(-10, 10)))
                    })
                    .collect(),
                Err(e) => {
                    tracing::warn!("社交事件 LLM 评估失败: {}", e);
                    return;
                }
            };

            // 记录事件（使用实体名称而非 UUID 字符串）
            // 预加载所有已知关系名字，避免循环内逐个查 DB
            let known_names: std::collections::HashMap<String, String> =
                match store.get_all_relationships() {
                    Ok(rels) => rels
                        .into_iter()
                        .filter(|r| !r.target_name.is_empty() && r.target_name != "陌生人")
                        .map(|r| (r.target_agent_id.to_string(), r.target_name))
                        .collect(),
                    Err(_) => std::collections::HashMap::new(),
                };

            for (i, event) in social_events.iter().enumerate() {
                let Some(meta) = event.metadata.as_object() else {
                    continue;
                };

                let action = meta.get("action").and_then(|v| v.as_str()).unwrap_or("");
                let is_speak = meta.contains_key("from_agent_id") && !meta.contains_key("action");
                let resolved_action = if is_speak { "speak" } else { action };
                let other_id_str = match resolved_action {
                    "给予" | "trade_sell" => meta.get("target").and_then(|v| v.as_str()),
                    "receive" | "trade_buy" | "stolen_from" => {
                        meta.get("from").and_then(|v| v.as_str())
                    }
                    _ => {
                        // PublicMessage: from_agent_id 标识说话者
                        meta.get("from_agent_id").and_then(|v| v.as_str())
                    }
                };

                let Some(id_str) = other_id_str else {
                    continue;
                };

                let Ok(other_id) = uuid::Uuid::parse_str(id_str) else {
                    continue;
                };

                let delta = deltas.get(&(i + 1)).copied().unwrap_or(0);
                // 名字解析：当前在线实体 → 已知关系存储 → "陌生人"
                let other_name = name_map
                    .get(id_str)
                    .cloned()
                    .or_else(|| known_names.get(id_str).cloned())
                    .unwrap_or_else(|| "陌生人".to_string());

                if let Err(e) = store.record_social_event(
                    other_id,
                    &other_name,
                    event.tick_id,
                    resolved_action,
                    &event.description,
                    delta,
                ) {
                    tracing::warn!("社交事件关系更新失败: {}", e);
                }
            }
        });
    }
}
