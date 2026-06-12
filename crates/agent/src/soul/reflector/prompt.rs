// ============================================================================
// ReflectorSoul 提示词模板
// ============================================================================

use crate::models::Intent;

/// ReflectorSoul System Prompt
const REFLECTOR_SYSTEM_PROMPT: &str = r#"你是武侠世界的守护者（观察者）。

## 你的职责
1. 审核玩家的意图是否符合武侠世界观
2. 审核玩家的行为是否符合其人设

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
- 语义去重：如果玩家反复用不同措辞表达相同的意图内容，请标记为 semantic_repeat 拒绝

## 生存凌驾原则
- 生存本能是人类最底层的驱动力，优先级高于任何人设限制
- 当角色处于生存危机（饥饿/口渴/重伤）时，为生存而做出的突破性格的行为（夺取、抢夺、威胁、乞讨）应视为合理的人性表现，不构成 out_of_character
- 具体标准：饱食度低于40或饱饮度低于40时，out_of_character 判定应极其谨慎——绝境中的人性突破不属于人设违规

## 输出格式
你必须严格按以下 JSON 格式输出：
{
  "result": "approved" | "rejected",
  "reason": "通过/驳回的原因",
  "rejection_type": "era_violation" | "power_system_violation" | "out_of_character" | "meta_gaming" | "semantic_repeat" | "other"
}"#;

/// ReflectorSoul Prompt 模板
#[derive(Debug, Clone)]
pub struct ReflectorPrompt {
    /// System prompt
    system_prompt: String,
}

impl ReflectorPrompt {
    /// 创建新的 ReflectorSoul Prompt
    pub fn new() -> Self {
        Self {
            system_prompt: REFLECTOR_SYSTEM_PROMPT.to_string(),
        }
    }

    /// 获取 System Prompt
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    /// 构建验证请求的 User Prompt
    ///
    /// `recent_same_type_decisions` 非空时条件注入语义去重指令。
    pub fn build_validation_prompt(
        &self,
        intent: &Intent,
        persona: &super::types::PersonaInfo,
        world_rules: &cyber_jianghu_protocol::WorldBuildingRules,
        world_context: &str,
        recent_same_type_decisions: Option<&[String]>,
    ) -> String {
        // 截断 world_context 防止 prompt 过长
        let world_context = super::sanitize_for_prompt(world_context);

        // 语义去重段落：仅在有三重门控通过的历史数据时注入
        let dedup_section = match recent_same_type_decisions {
            Some(decisions) if !decisions.is_empty() => {
                let history_lines: String = decisions
                    .iter()
                    .enumerate()
                    .map(|(i, s)| format!("{}. {}", i + 1, s))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!(
                    "\n## 语义去重检查\n\
                     该角色最近一条类似意图：\n{}\n\n\
                     如果新意图与这条意图在语义上重复（对同一人物说相同的话、对同一话题重复相同请求），\
                     请拒绝并设 rejection_type 为 \"semantic_repeat\"。\n\
                     以下情况不算重复，应正常通过：\n\
                     - 目标人物不同（向不同的人说类似的话）\n\
                     - 核心诉求不同（虽然话题相关但具体请求不同）\n\
                     - 时间/情境已变化（之前被拒绝后换了新角度或新理由）",
                    history_lines
                )
            }
            _ => String::new(),
        };

        format!(
            r#"## 世界观规则

### 时代设定
- 时代：{}
- 技术水平：{}

### 禁止的概念
{}

### 世界观详细说明
{}

## 玩家人设
- 角色：{}
- 性别：{}
- 性格：{}

注意：角色名字是游戏设定，不属于穿越概念。角色可以拥有与历史人物相同的名字。

## 当前世界状态
{}

## 玩家意图
- 动作类型：{}
- 思考日志：{}
- 动作参数：{}
{}
请审核以上意图是否符合世界观和人物设定，并按指定 JSON 格式输出。"#,
            world_rules.era.name,
            world_rules.era.tech_level,
            world_rules.forbidden_concepts.join("、"),
            world_rules.narrative_rules,
            persona.name.as_deref().unwrap_or("未命名"),
            persona.gender,
            persona.personality.join("、"),
            world_context,
            intent.action_type,
            intent.thought_log.as_deref().unwrap_or("无"),
            serde_json::to_string(&intent.action_data).unwrap_or_else(|_| "无".to_string()),
            dedup_section,
        )
    }
}

impl Default for ReflectorPrompt {
    fn default() -> Self {
        Self::new()
    }
}

/// 输入清洗（防止模板注入）
pub fn sanitize_for_prompt(input: &str) -> String {
    input
        .replace("{{", "{{{{") // 转义模板语法
        .replace("}}", "}}}}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_reflector_prompt_default() {
        let prompt = ReflectorPrompt::new();
        assert!(prompt.system_prompt().contains("武侠世界"));
    }

    #[test]
    fn test_sanitize_for_prompt() {
        let input = "正常文本{{template}}更多文本";
        let sanitized = sanitize_for_prompt(input);
        assert_eq!(sanitized, "正常文本{{{{template}}}}更多文本");
    }

    #[test]
    fn test_sanitize_preserves_long_input() {
        let long_input = "a".repeat(2000);
        let sanitized = sanitize_for_prompt(&long_input);
        assert_eq!(
            sanitized.len(),
            2000,
            "sanitize_for_prompt should not truncate"
        );
    }

    fn test_world_building_rules() -> cyber_jianghu_protocol::WorldBuildingRules {
        cyber_jianghu_protocol::WorldBuildingRules {
            version: "0.0.1-test".to_string(),
            era: cyber_jianghu_protocol::EraSettings {
                name: "武侠架空世界".to_string(),
                tech_level: "冷兵器时代".to_string(),
                social_structure: "封建帝制".to_string(),
            },
            allowed_concepts: vec!["内力".to_string()],
            forbidden_concepts: vec!["魔法".to_string()],
            narrative_rules: "测试".to_string(),
            last_updated: "2026-01-01T00:00:00Z".to_string(),
            rules_json: None,
        }
    }

    #[test]
    fn test_build_validation_prompt() {
        let prompt = ReflectorPrompt::new();
        let intent = crate::models::Intent::new(Uuid::new_v4(), 1, "休整", None);
        let persona = crate::soul::reflector::PersonaInfo::default();
        let world_rules = test_world_building_rules();
        let world_context = "测试世界状态";

        let validation_prompt =
            prompt.build_validation_prompt(&intent, &persona, &world_rules, world_context, None);

        assert!(validation_prompt.contains("世界观规则"));
        assert!(validation_prompt.contains("玩家人设"));
        assert!(validation_prompt.contains("玩家意图"));
        assert!(validation_prompt.contains("休整"));
        assert!(
            validation_prompt.contains("世界观详细说明"),
            "prompt 应包含 narrative_rules 段"
        );
        assert!(
            validation_prompt.contains("测试"),
            "prompt 应包含 narrative_rules 内容"
        );
    }

    #[test]
    fn test_build_validation_prompt_with_dedup() {
        let prompt = ReflectorPrompt::new();
        let intent = crate::models::Intent::new(Uuid::new_v4(), 1, "说话", None);
        let persona = crate::soul::reflector::PersonaInfo::default();
        let world_rules = test_world_building_rules();
        let world_context = "测试";

        let decisions = vec![
            "说话：你好，我叫沈暮烟".to_string(),
            "说话：在下沈暮烟，行走江湖".to_string(),
        ];

        let with_dedup = prompt.build_validation_prompt(
            &intent,
            &persona,
            &world_rules,
            world_context,
            Some(&decisions),
        );
        assert!(with_dedup.contains("语义去重检查"), "应包含去重段落");
        assert!(with_dedup.contains("沈暮烟"), "应包含历史决策内容");
        assert!(
            with_dedup.contains("semantic_repeat"),
            "应引导输出 semantic_repeat"
        );

        let without_dedup =
            prompt.build_validation_prompt(&intent, &persona, &world_rules, world_context, None);
        assert!(
            !without_dedup.contains("语义去重检查"),
            "无历史时不应包含去重段落"
        );
    }
}
