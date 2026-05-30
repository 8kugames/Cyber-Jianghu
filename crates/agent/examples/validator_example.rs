//! IntentValidator 使用示例
//!
//! 演示如何使用 IntentValidator 进行意图验证
//!
//! 运行方式：
//! ```bash
//! cargo run -p cyber-jianghu-agent --example validator_example
//! ```

use cyber_jianghu_agent::{LlmClient, PersonaInfo, ReflectorSoul, Validator};
use cyber_jianghu_protocol::{EraSettings, WorldBuildingRules};
use std::sync::Arc;
use tokio::sync::RwLock;

// ============================================================================
// Mock LLM 客户端（仅用于示例）
// ============================================================================

/// 示例 LLM 客户端
///
/// 实际使用时，OpenClaw 会提供真实的 LLM 客户端实现
struct ExampleLlmClient;

#[async_trait::async_trait]
impl LlmClient for ExampleLlmClient {
    async fn complete(&self, _prompt: &str) -> anyhow::Result<String> {
        // 模拟 LLM 响应 - 总是返回批准
        Ok(r#"{
            "result": "approved",
            "reason": "行为符合武侠世界观",
            "narrative": "角色决定在客栈休息片刻，观察周围动静。"
        }"#
        .to_string())
    }

    async fn complete_with_system(&self, _system: &str, _prompt: &str) -> anyhow::Result<String> {
        self.complete(_prompt).await
    }
}

// ============================================================================
// 示例
// ============================================================================

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 初始化日志
    tracing_subscriber::fmt::init();

    println!("=== ReflectorSoul 示例 ===\n");

    // 1. 创建 WorldBuildingRules（示例配置）
    let world_rules = WorldBuildingRules {
        version: "0.0.1-example".to_string(),
        era: EraSettings {
            name: "武侠架空世界".to_string(),
            tech_level: "冷兵器时代".to_string(),
            social_structure: "封建帝制".to_string(),
        },
        allowed_concepts: vec!["内力".to_string(), "轻功".to_string()],
        forbidden_concepts: vec!["魔法".to_string(), "仙术".to_string()],
        narrative_rules: "示例叙事规则".to_string(),
        last_updated: "2026-01-01T00:00:00Z".to_string(),
        rules_json: None,
    };
    println!("世界观规则版本: {}", world_rules.version);
    println!("   时代: {}", world_rules.era.name);
    println!("   技术水平: {}", world_rules.era.tech_level);
    println!("   允许概念: {:?}", world_rules.allowed_concepts);
    println!("   禁止概念: {:?}\n", world_rules.forbidden_concepts);

    // 2. 创建 LLM 客户端（实际使用时由 OpenClaw 提供）
    let llm_client = Arc::new(ExampleLlmClient);

    // 3. 创建 ReflectorSoul（意图审查引擎）
    let validator: Arc<dyn Validator> = Arc::new(ReflectorSoul::new(
        world_rules.clone(),
        Arc::new(RwLock::new(llm_client)),
    ));
    println!("ReflectorSoul 已创建\n");

    // 4. 模拟验证流程
    println!("=== 模拟验证 ===\n");

    // 创建人设信息
    let persona = PersonaInfo::default();
    println!("人设信息:");
    println!("   性别: {}", persona.gender);
    println!("   年龄: {}", persona.age);
    println!("   性格: {:?}", persona.personality);
    println!("   价值观: {:?}\n", persona.values);

    // 验证人设
    println!("验证人设...");
    match validator.validate_persona(&persona).await? {
        cyber_jianghu_agent::ValidationResult::Approved { reason, narrative } => {
            println!("人设验证通过");
            if let Some(r) = reason {
                println!("   原因: {}", r);
            }
            if !narrative.is_empty() {
                println!("   叙事: {}", narrative);
            }
        }
        cyber_jianghu_agent::ValidationResult::Rejected {
            reason,
            rejection_type,
        } => {
            println!("人设验证失败: {} [{:?}]", reason, rejection_type);
        }
    }

    // 5. 完成
    println!("\n示例完成！\n");
    println!("提示：实际使用时，需要：");
    println!("1. 从 OpenClaw 获取 LlmClient 实现");
    println!("2. 从服务端获取 WorldBuildingRules");
    println!("3. 将验证器注入到 Agent 中：");
    println!(
        "   let validator: Arc<dyn Validator> = Arc::new(ReflectorSoul::new(rules, Arc::new(RwLock::new(llm_client))));"
    );
    println!("   agent.set_intent_auditor(validator);");

    Ok(())
}
