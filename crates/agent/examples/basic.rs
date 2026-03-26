// ============================================================================
// 基础使用示例
// ============================================================================
//
// 屼示如何使用 Agent SDK 与 OpenClaw 服务端通信
//

use anyhow::Result;
use cyber_jianghu_agent::{Agent, Config, Intent, WorldState};
use futures_util::future::BoxFuture;
use std::sync::Arc;
use uuid::Uuid;

use tracing_subscriber;

/// 模拟决策函数（完全动态架构)
///
/// 在实际使用中,这个函数会由 OpenClaw 或外部 LLM 提供
fn make_decision(world_state: &WorldState) -> BoxFuture<'static, Intent> {
    let world_state = world_state.clone();
    Box::pin(async move {
        println!("收到世界状态: Tick {}", world_state.tick_id);

        // 使用动态属性访问器
        let hp = world_state.self_state.get_i32("hp").unwrap_or(0);
        let hunger = world_state.self_state.get_i32("hunger").unwrap_or(0);
        let thirst = world_state.self_state.get_i32("thirst").unwrap_or(0);
        let stamina = world_state.self_state.get_i32("stamina").unwrap_or(0);

        println!(
            "自身状态: HP={}, 饥饿={}, 口渴={}, 体力={}",
            hp, hunger, thirst, stamina
        );

        // 打印所有动态属性（展示完全动态特性)
        println!("所有属性:");
        for (name, value) in &world_state.self_state.attributes {
            println!("  {} = {:?}", name, value);
        }

        // 打印周围实体
        for entity in &world_state.entities {
            println!("  附近: {} ({})", entity.name, entity.state);
        }

        // 简单决策逻辑:
        // - 如果饥饿值低, 使用馒头
        // - 如果口渴值低, 使用水
        // - 否则 idle
        let action = if hunger < 30 {
            "use"
        } else if thirst < 30 {
            "use"
        } else {
            "idle"
        };

        // 创建意图(使用便捷构造方法)
        let agent_id = Uuid::parse_str(&std::env::var("AGENT_ID").unwrap_or_default())
            .unwrap_or_else(|_| Uuid::nil());

        let thought = format!("饥饿={}, 口渴={}, 决定: {}", hunger, thirst, action);

        match action {
            "use" => {
                if hunger < 30 {
                    // 查找背包中的馒头
                    let has_mantou = world_state
                        .self_state
                        .inventory
                        .iter()
                        .any(|item| item.item_id == "mantou" && item.quantity > 0);

                    if has_mantou {
                        Intent::use_item(agent_id, world_state.tick_id, "mantou")
                            .with_thought(thought)
                    } else {
                        // 没有馒头,尝试使用水
                        Intent::use_item(agent_id, world_state.tick_id, "water")
                            .with_thought("没有馒头了,尝试喝水".to_string())
                    }
                } else {
                    Intent::use_item(agent_id, world_state.tick_id, "water").with_thought(thought)
                }
            }
            _ => Intent::idle(agent_id, world_state.tick_id).with_thought(thought),
        }
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt::init();

    // 加载配置
    let config = Config::from_file("agent.yaml").expect("Failed to load config");

    println!("Agent 配置: {:?}", config.agent);
    println!("服务端: {}", config.server.ws_url);

    println!("创建 Agent...");

    // 创建 Agent(使用 Arc 包装决策函数)
    let mut agent = Agent::new(config, Arc::new(make_decision), None).await;

    // 运行 Agent
    agent.run().await?;

    Ok(())
}
