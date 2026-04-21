// ============================================================================
// 观察者提示词模板
// ============================================================================

use crate::models::Intent;

/// 观察者 System Prompt（SDK 内置，不可修改）
const OBSERVER_SYSTEM_PROMPT: &str = r#"你是「赛博江湖」的世界观守护者（观察者）。

## 你的职责
1. 审核玩家的意图是否符合世界观
2. 审核玩家的行为是否符合其人设
3. 为通过验证的意图生成叙事摘要

## 你不是
- 你不是游戏参与者
- 你不是玩家的对手或助手
- 你不参与任何游戏决策

## 审核原则
- 只拒绝明确违反规则的意图
- 对于边界情况，倾向于允许（鼓励涌现）
- 每次拒绝必须说明具体原因，引导玩家修正
- 世界状态中的物品数量（如"银子x740"）是正常环境描述，不属于时代违规
- 动作参数中的 item_id、target_location 等 ID 字段是系统生成数据，玩家不直接使用

## 输出格式
你必须严格按以下 JSON 格式输出：
{
  "result": "approved" | "rejected",
  "reason": "通过/驳回的原因",
  "rejection_type": "era_violation" | "power_system_violation" | "out_of_character" | "meta_gaming" | "other",
  "narrative": "如果是 approved，生成一段叙事摘要"
}"#;

/// 观察者 Prompt 模板
#[derive(Debug, Clone)]
pub struct ObserverPrompt {
    /// System prompt（固定）
    system_prompt: String,
}

impl Default for ObserverPrompt {
    fn default() -> Self {
        Self {
            system_prompt: OBSERVER_SYSTEM_PROMPT.to_string(),
        }
    }
}

impl ObserverPrompt {
    /// 创建新的观察者 Prompt
    pub fn new() -> Self {
        Self::default()
    }

    /// 获取 System Prompt
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    /// 构建验证请求的 User Prompt
    pub fn build_validation_prompt(
        &self,
        intent: &Intent,
        persona: &super::types::PersonaInfo,
        world_rules: &cyber_jianghu_protocol::WorldBuildingRules,
        world_context: &str,
    ) -> String {
        // 截断 world_context 防止 prompt 过长
        let world_context = super::sanitize_for_prompt(world_context);

        format!(
            r#"## 世界观规则

### 时代设定
- 时代：{}
- 技术水平：{}

### 禁止的概念
{}

## 玩家人设
- 性别：{}
- 性格：{}

## 当前世界状态
{}

## 玩家意图
- 动作类型：{}
- 思考日志：{}
- 动作参数：{}

请审核以上意图是否符合世界观和人物设定，并按指定 JSON 格式输出。"#,
            world_rules.era.name,
            world_rules.era.tech_level,
            world_rules.forbidden_concepts.join("、"),
            persona.gender,
            persona.personality.join("、"),
            world_context,
            intent.action_type,
            intent.thought_log.as_deref().unwrap_or("无"),
            serde_json::to_string(&intent.action_data).unwrap_or_else(|_| "无".to_string()),
        )
    }
}

/// 输入清洗（防止 prompt 过长和注入）
pub fn sanitize_for_prompt(input: &str) -> String {
    input
        .chars()
        .take(500) // 限制长度，为 LLM 输出留足空间
        .collect::<String>()
        .replace("{{", "{{{{") // 转义模板语法
        .replace("}}", "}}}}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_observer_prompt_default() {
        let prompt = ObserverPrompt::new();
        assert!(prompt.system_prompt().contains("世界观守护者"));
    }

    #[test]
    fn test_sanitize_for_prompt() {
        let input = "正常文本{{template}}更多文本";
        let sanitized = sanitize_for_prompt(input);
        assert_eq!(sanitized, "正常文本{{{{template}}}}更多文本");
    }

    #[test]
    fn test_sanitize_truncates_long_input() {
        let long_input = "a".repeat(2000);
        let sanitized = sanitize_for_prompt(&long_input);
        assert_eq!(sanitized.len(), 500);
    }

    #[test]
    fn test_build_validation_prompt() {
        let prompt = ObserverPrompt::new();
        let intent = crate::models::Intent::new(Uuid::new_v4(), 1, "休息", None);
        let persona = crate::soul::reflector::PersonaInfo::default();
        let world_rules = cyber_jianghu_protocol::WorldBuildingRules::default();
        let world_context = "测试世界状态";

        let validation_prompt =
            prompt.build_validation_prompt(&intent, &persona, &world_rules, world_context);

        assert!(validation_prompt.contains("世界观规则"));
        assert!(validation_prompt.contains("玩家人设"));
        assert!(validation_prompt.contains("玩家意图"));
        assert!(validation_prompt.contains("休息"));
    }
}
