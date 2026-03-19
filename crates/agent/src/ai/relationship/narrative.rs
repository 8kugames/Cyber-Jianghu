// ============================================================================
// 叙事化描述生成模块
// ============================================================================
//
// 使用 LLM 生成个性化的好感度叙事化描述

use crate::ai::llm::LlmClient;
use crate::ai::persona::DynamicPersona;
use crate::ai::relationship::store::RelationshipStore;
use crate::ai::relationship::types::RelationshipMemory;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

/// 叙事化描述生成器
pub struct NarrativeGenerator {
    llm_client: Arc<dyn LlmClient>,
    /// 待生成/进行中的任务 {target_id -> tick}
    pending_generations: Arc<Mutex<HashMap<Uuid, i64>>>,
}

impl NarrativeGenerator {
    /// 创建新的叙事生成器
    pub fn new(llm_client: Arc<dyn LlmClient>) -> Self {
        Self {
            llm_client,
            pending_generations: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 异步生成好感度描述
    pub async fn generate_description(
        &self,
        memory: &RelationshipMemory,
        agent_persona: &DynamicPersona,
    ) -> Result<String> {
        // 构建 Prompt
        let prompt = self.build_prompt(memory, agent_persona);

        // 调用 LLM（带超时和重试）
        let response =
            tokio::time::timeout(Duration::from_secs(30), self.llm_client.complete(&prompt))
                .await
                .map_err(|_| anyhow::anyhow!("LLM 调用超时"))?
                .map_err(|e| anyhow::anyhow!("LLM 调用失败: {}", e))?;

        // 提取并验证描述（20字以内）
        let description = self.extract_description(&response)?;
        Ok(description)
    }

    /// 构建 Prompt（使用 DynamicPersona 获取特质）
    fn build_prompt(&self, memory: &RelationshipMemory, persona: &DynamicPersona) -> String {
        let event_summary = self.summarize_events(memory);
        let traits_summary = self.format_persona_traits(persona);

        format!(
            "你是一个武侠世界的角色。请根据以下信息，用20字以内描述你对目标人物的好感度：\n\
             \n\
             - 好感度：{}（-100深仇大恨，100生死之交）\n\
             - 最近互动摘要：{}\n\
             - 你的人设特质：{}\n\
             \n\
             要求：\n\
             - 情感+程度风格，如\"一位值得信赖的挚友，多次相助让我心怀感激\"\n\
             - 20字以内\n\
             - 第一人称视角\n\
             \n\
             描述：",
            memory.favorability, event_summary, traits_summary
        )
    }

    /// 格式化人设特质
    fn format_persona_traits(&self, persona: &DynamicPersona) -> String {
        // 取最重要的 3 个特质
        let trait_items: Vec<_> = persona
            .traits
            .iter()
            .filter(|(_, t)| t.value() >= 60 || t.value() <= 40)
            .take(3)
            .map(|(name, t)| format!("{}({})", name, t.narrative_description()))
            .collect();

        if trait_items.is_empty() {
            "性格平和".to_string()
        } else {
            trait_items.join("、")
        }
    }

    /// 提取并验证描述
    fn extract_description(&self, response: &str) -> Result<String> {
        let description = response
            .trim()
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.trim())
            .next()
            .unwrap_or("");

        let description = description
            .strip_prefix("描述：")
            .or(description.strip_prefix("描述:"))
            .unwrap_or(description);

        let description = description
            .trim()
            .strip_suffix('"')
            .unwrap_or(description)
            .trim()
            .strip_prefix('"')
            .unwrap_or(description)
            .trim();

        // 验证长度
        let char_count = description.chars().count();
        if char_count > 20 {
            // 截断到 20 字
            Ok(description.chars().take(20).collect())
        } else if char_count == 0 {
            // fallback 到默认描述
            Ok("素不相识".to_string())
        } else {
            Ok(description.to_string())
        }
    }

    /// 智能摘要事件历史
    fn summarize_events(&self, memory: &RelationshipMemory) -> String {
        let recent = memory.get_recent_events(5);
        if recent.is_empty() {
            return "暂无互动".to_string();
        }

        let positive_count = recent.iter().filter(|e| e.favorability_delta > 0).count();
        let negative_count = recent.iter().filter(|e| e.favorability_delta < 0).count();
        let total = recent.len();

        if positive_count > negative_count * 2 {
            format!("最近{}次互动多为正面", total)
        } else if negative_count > positive_count * 2 {
            format!("最近{}次互动多为负面", total)
        } else {
            format!("最近{}次互动喜忧参半", total)
        }
    }

    /// 带去重的异步更新（带错误处理）
    pub async fn update_with_debounce(
        &self,
        target_id: Uuid,
        current_tick: i64,
        memory: &RelationshipMemory,
        persona: &DynamicPersona,
        store: &RelationshipStore,
    ) -> Result<()> {
        // 检查是否需要更新（避免重复生成）
        {
            let mut pending = self
                .pending_generations
                .lock()
                .map_err(|e| anyhow::anyhow!("Lock error: {}", e))?;
            if let Some(&last_tick) = pending.get(&target_id)
                && last_tick == current_tick {
                    return Ok(()); // 已在当前 tick 生成过
                }
            pending.insert(target_id, current_tick);
        }

        // 生成新描述（失败时返回 Ok，不影响主流程）
        match self.generate_description(memory, persona).await {
            Ok(new_description) => {
                if let Err(e) =
                    store.update_self_description(target_id, &new_description, current_tick)
                {
                    tracing::warn!("[narrative] 更新描述失败: {}", e);
                }
            }
            Err(e) => {
                tracing::warn!("[narrative] 生成描述失败: {}", e);
                // 失败时不更新，下次重试
            }
        }

        Ok(())
    }
}

// ============================================================================
// NarrativeGenerator 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::llm::MockLlmClient;
    use crate::ai::persona::dynamic_persona::DynamicPersona;
    use std::sync::Arc;
    use uuid::Uuid;

    fn create_test_memory(favorability: i32) -> RelationshipMemory {
        let mut memory = RelationshipMemory::new(Uuid::new_v4(), "测试目标");
        memory.set_favorability(favorability);
        memory
    }

    fn create_test_persona() -> DynamicPersona {
        DynamicPersona::new(Uuid::new_v4(), "测试角色", "一个测试用的角色")
    }

    #[test]
    fn test_extract_description_short() {
        let client = MockLlmClient::with_response("一位值得信赖的挚友");
        let generator = NarrativeGenerator::new(Arc::new(client));
        let result = generator.extract_description("一位值得信赖的挚友").unwrap();
        assert_eq!(result, "一位值得信赖的挚友");
    }

    #[test]
    fn test_extract_description_with_prefix() {
        let client = MockLlmClient::with_response("描述：一位值得信赖的挚友");
        let generator = NarrativeGenerator::new(Arc::new(client));
        let result = generator
            .extract_description("描述：一位值得信赖的挚友")
            .unwrap();
        assert_eq!(result, "一位值得信赖的挚友");
    }

    #[test]
    fn test_extract_description_with_quotes() {
        let client = MockLlmClient::with_response("\"一位值得信赖的挚友\"");
        let generator = NarrativeGenerator::new(Arc::new(client));
        let result = generator
            .extract_description("\"一位值得信赖的挚友\"")
            .unwrap();
        assert_eq!(result, "一位值得信赖的挚友");
    }

    #[test]
    fn test_extract_description_too_long() {
        let long_desc = "这是一个非常非常非常非常非常非常非常非常长的描述";
        assert!(long_desc.chars().count() > 20);
        let client = MockLlmClient::with_response(long_desc);
        let generator = NarrativeGenerator::new(Arc::new(client));
        let result = generator.extract_description(long_desc).unwrap();
        assert_eq!(result.chars().count(), 20);
        // 验证内容被截断而非默认值
        assert_ne!(result, "素不相识");
    }

    #[test]
    fn test_extract_description_empty() {
        let client = MockLlmClient::with_response("");
        let generator = NarrativeGenerator::new(Arc::new(client));
        let result = generator.extract_description("").unwrap();
        assert_eq!(result, "素不相识");
    }

    #[test]
    fn test_summarize_events_no_events() {
        let client = MockLlmClient::with_response("test");
        let generator = NarrativeGenerator::new(Arc::new(client));
        let memory = create_test_memory(50);
        let summary = generator.summarize_events(&memory);
        assert_eq!(summary, "暂无互动");
    }

    #[test]
    fn test_summarize_events_positive() {
        let client = MockLlmClient::with_response("test");
        let generator = NarrativeGenerator::new(Arc::new(client));
        let mut memory = create_test_memory(50);
        memory.add_event(crate::ai::relationship::types::KeyEvent::new(
            1, "help", "帮助", 10,
        ));
        memory.add_event(crate::ai::relationship::types::KeyEvent::new(
            2, "gift", "礼物", 5,
        ));
        memory.add_event(crate::ai::relationship::types::KeyEvent::new(
            3, "help", "帮助", 5,
        ));
        let summary = generator.summarize_events(&memory);
        assert_eq!(summary, "最近3次互动多为正面");
    }

    #[test]
    fn test_summarize_events_negative() {
        let client = MockLlmClient::with_response("test");
        let generator = NarrativeGenerator::new(Arc::new(client));
        let mut memory = create_test_memory(50);
        memory.add_event(crate::ai::relationship::types::KeyEvent::new(
            1, "attack", "攻击", -10,
        ));
        memory.add_event(crate::ai::relationship::types::KeyEvent::new(
            2, "insult", "侮辱", -5,
        ));
        memory.add_event(crate::ai::relationship::types::KeyEvent::new(
            3, "attack", "攻击", -5,
        ));
        let summary = generator.summarize_events(&memory);
        assert_eq!(summary, "最近3次互动多为负面");
    }

    #[test]
    fn test_summarize_events_mixed() {
        let client = MockLlmClient::with_response("test");
        let generator = NarrativeGenerator::new(Arc::new(client));
        let mut memory = create_test_memory(50);
        memory.add_event(crate::ai::relationship::types::KeyEvent::new(
            1, "help", "帮助", 10,
        ));
        memory.add_event(crate::ai::relationship::types::KeyEvent::new(
            2, "attack", "攻击", -10,
        ));
        let summary = generator.summarize_events(&memory);
        assert_eq!(summary, "最近2次互动喜忧参半");
    }

    #[test]
    fn test_format_persona_traits_empty() {
        let client = MockLlmClient::with_response("test");
        let generator = NarrativeGenerator::new(Arc::new(client));
        let mut persona = create_test_persona();
        persona.traits.clear();
        let formatted = generator.format_persona_traits(&persona);
        assert_eq!(formatted, "性格平和");
    }

    #[tokio::test]
    async fn test_generate_description_with_llm() {
        let client = MockLlmClient::with_response("一位值得信赖的挚友");
        let generator = NarrativeGenerator::new(Arc::new(client));
        let memory = create_test_memory(80);
        let persona = create_test_persona();

        let result = generator
            .generate_description(&memory, &persona)
            .await
            .unwrap();
        assert_eq!(result, "一位值得信赖的挚友");
    }

    #[tokio::test]
    async fn test_generate_description_fallback_on_empty() {
        let client = MockLlmClient::with_response("");
        let generator = NarrativeGenerator::new(Arc::new(client));
        let memory = create_test_memory(0);
        let persona = create_test_persona();

        let result = generator
            .generate_description(&memory, &persona)
            .await
            .unwrap();
        // 空描述应该 fallback 到默认值
        assert_eq!(result, "素不相识");
    }
}
