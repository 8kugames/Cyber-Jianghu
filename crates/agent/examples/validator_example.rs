//! IntentValidator 使用示例
//!
//! 演示如何使用 IntentValidator 进行意图验证
//!
//! 运行方式：
//! ```bash
//! cargo run -p cyber-jianghu-agent --example validator_example
//! ```

use cyber_jianghu_agent::{
    IntentValidator, LifespanCalculator, LifespanConfig, LlmClient, PersonaInfo, Validator,
};
use cyber_jianghu_protocol::WorldBuildingRules;
use std::sync::Arc;

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
}

// ============================================================================
// 示例
// ============================================================================

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 初始化日志
    tracing_subscriber::fmt::init();

    println!("=== IntentValidator 示例 ===\n");

    // 1. 创建 WorldBuildingRules
    let world_rules = WorldBuildingRules::default();
    println!("世界观规则版本: {}", world_rules.version);
    println!("   时代: {}", world_rules.era.name);
    println!("   技术水平: {}", world_rules.era.tech_level);
    println!("   允许概念: {:?}", world_rules.allowed_concepts);
    println!("   禁止概念: {:?}\n", world_rules.forbidden_concepts);

    // 2. 创建 LLM 客户端（实际使用时由 OpenClaw 提供）
    let llm_client = Arc::new(ExampleLlmClient);

    // 3. 创建 IntentValidator
    // 注意：IntentValidator 是泛型的，需要指定 LLM 客户端类型
    let validator: Arc<dyn Validator> =
        Arc::new(IntentValidator::new(world_rules.clone(), llm_client, None));
    println!("IntentValidator 已创建\n");

    // 4. 创建 LifespanCalculator
    let lifespan_config = LifespanConfig {
        initial_age: 28,
        max_age: 80,
        ..Default::default()
    };
    let mut lifespan = LifespanCalculator::new(lifespan_config);
    println!("初始年龄: {}", lifespan.current_age());
    println!("   叙事描述: {}\n", lifespan.get_narrative_description());

    // 5. 模拟验证流程
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

    // 6. 模拟年龄增长
    println!("\n=== 模拟年龄增长 ===\n");

    for i in 1..=5 {
        let status = lifespan.process_tick();
        println!(
            "Tick {}: 年龄 {} - {}",
            i,
            status.age(),
            lifespan.get_narrative_description()
        );
    }

    // 7. 显示最终状态
    println!("\n=== 最终状态 ===\n");
    let final_status = lifespan.get_status();
    println!("年龄: {}", final_status.age());
    println!(
        "状态: {}",
        if final_status.is_alive() {
            "存活"
        } else {
            "已故"
        }
    );

    println!("\n示例完成！\n");
    println!("提示：实际使用时，需要：");
    println!("1. 从 OpenClaw 获取 LlmClient 实现");
    println!("2. 从服务端获取 WorldBuildingRules");
    println!("3. 将验证器注入到 Agent 中：");
    println!(
        "   let validator: Arc<dyn Validator> = Arc::new(IntentValidator::new(rules, llm_client));"
    );
    println!("   agent.set_validator(validator);");

    Ok(())
}
