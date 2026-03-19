//! 对话系统示例
//!
//! 展示如何使用对话系统与其他 Agent 进行交流。

use anyhow::Result;
use std::sync::Arc;
use tokio::time::{Duration, sleep};
use tracing::{info, warn};
use uuid::Uuid;

use cyber_jianghu_agent::ai::dialogue::{DialogueClient, DialogueEventHandler};
use cyber_jianghu_agent::ai::relationship::RelationshipStore;
use cyber_jianghu_agent::{Agent, Config};

// ============================================================================
// SimpleDialogueHandler
// ============================================================================

/// 简单的对话事件处理器
///
/// 实现基本的对话功能，包括接受请求、发送消息、结束对话。
struct SimpleDialogueHandler {
    agent_name: String,
}

impl SimpleDialogueHandler {
    fn new(agent_name: String) -> Self {
        Self { agent_name }
    }
}

impl DialogueEventHandler for SimpleDialogueHandler {
    fn on_dialogue_request(&self, from_agent_id: Uuid, opening_remark: String) {
        info!(
            "[{}] 收到来自 {} 的对话请求: {}",
            self.agent_name, from_agent_id, opening_remark
        );
        // 在实际应用中，这里会调用 dialogue_client.accept_dialogue() 或 reject_dialogue()
        // 本示例中，我们在主循环中手动处理
    }

    fn on_dialogue_accepted(&self, session_id: String) {
        info!("[{}] 对话被接受，会话 ID: {}", self.agent_name, session_id);
    }

    fn on_dialogue_rejected(&self, session_id: String, reason: Option<String>) {
        warn!(
            "[{}] 对话被拒绝，会话 ID: {}，原因: {:?}",
            self.agent_name, session_id, reason
        );
    }

    fn on_dialogue_message(&self, session_id: String, from_agent_id: Uuid, content: String) {
        info!(
            "[{}] 收到来自 {} 的消息 [{}]: {}",
            self.agent_name, from_agent_id, session_id, content
        );
    }

    fn on_dialogue_ended(&self, session_id: String, by_agent: Uuid) {
        info!(
            "[{}] 对话结束，会话 ID: {}，由 {} 结束",
            self.agent_name, session_id, by_agent
        );
    }
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    info!("启动对话系统示例");

    // 创建第一个 Agent 的配置
    let config1 = Config {
        agent: cyber_jianghu_agent::AgentConfig {
            name: "张三".to_string(),
            system_prompt: "你是一个武侠世界的侠客。".to_string(),
            persona: Default::default(),
            memory: Default::default(),
            role: Default::default(),
            review: None,
            observer: None,
        },
        server: cyber_jianghu_agent::ServerConfig {
            ws_url: "ws://localhost:23333/ws".to_string(),
            auth_token: "test-token-1".to_string(),
        },
        memory: Default::default(),
        game_rules: None,
    };

    // 创建第二个 Agent 的配置
    let config2 = Config {
        agent: cyber_jianghu_agent::AgentConfig {
            name: "李四".to_string(),
            system_prompt: "你是一个武侠世界的商人。".to_string(),
            persona: Default::default(),
            memory: Default::default(),
            role: Default::default(),
            review: None,
            observer: None,
        },
        server: cyber_jianghu_agent::ServerConfig {
            ws_url: "ws://localhost:23333/ws".to_string(),
            auth_token: "test-token-2".to_string(),
        },
        memory: Default::default(),
        game_rules: None,
    };

    use futures_util::future::BoxFuture;

    // 创建决策回调（简单的空闲决策）
    let decision_callback1 = Arc::new(
        |_: &cyber_jianghu_agent::WorldState| -> BoxFuture<'static, cyber_jianghu_agent::Intent> {
            Box::pin(async { cyber_jianghu_agent::Intent::idle(Uuid::new_v4(), 0) })
        },
    );
    let decision_callback2 = Arc::new(
        |_: &cyber_jianghu_agent::WorldState| -> BoxFuture<'static, cyber_jianghu_agent::Intent> {
            Box::pin(async { cyber_jianghu_agent::Intent::idle(Uuid::new_v4(), 0) })
        },
    );

    // 创建 Agents
    let mut agent1 = Agent::new(config1, decision_callback1);
    let mut agent2 = Agent::new(config2, decision_callback2);

    // 初始化对话处理器
    let handler1 = Arc::new(SimpleDialogueHandler::new("张三".to_string()));
    let handler2 = Arc::new(SimpleDialogueHandler::new("李四".to_string()));

    // 注意：在实际使用中，需要先连接并获取 agent_id，然后创建 DialogueClient
    // 这里我们演示如何设置

    info!("对话系统示例初始化完成");
    info!("注意：此示例展示了如何设置对话系统");
    info!("   实际使用时，需要在连接后获取 agent_id，然后创建 DialogueClient");

    // 演示对话流程（伪代码）
    info!("\n对话流程示例：");
    info!("1. Agent 1 请求与 Agent 2 对话");
    info!(
        "   let message = dialogue_client1.request_dialogue(agent2_id, \"你好，能聊聊吗？\".to_string());"
    );
    info!("   client1.send_dialogue(message).await?;");
    info!("");
    info!("2. Agent 2 收到请求并接受");
    info!("   // 在 on_dialogue_request 中处理");
    info!("   let accept_msg = dialogue_client2.accept_dialogue(session_id);");
    info!("   client2.send_dialogue(accept_msg).await?;");
    info!("");
    info!("3. Agent 1 发送消息");
    info!(
        "   let msg = dialogue_client1.send_message(session_id, \"最近生意怎么样？\".to_string());"
    );
    info!("   client1.send_dialogue(msg).await?;");
    info!("");
    info!("4. Agent 2 收到消息并回复");
    info!("   // 在 on_dialogue_message 中处理");
    info!(
        "   let reply = dialogue_client2.send_message(session_id, \"还不错，多谢关心。\".to_string());"
    );
    info!("   client2.send_dialogue(reply).await?;");
    info!("");
    info!("5. Agent 1 结束对话");
    info!("   let end_msg = dialogue_client1.end_dialogue(session_id);");
    info!("   client1.send_dialogue(end_msg).await?;");

    // 演示关系系统
    info!("\n关系系统示例：");
    info!("// 创建关系存储");
    info!("let relationship_store = RelationshipStore::new(agent_id)?;");
    info!("");
    info!("// 记录正面互动");
    info!("relationship_store.record_interaction(");
    info!("    target_agent_id,");
    info!("    KeyEvent::Positive {{");
    info!("        event_type: \"dialogue\".to_string(),");
    info!("        description: \"愉快的对话\".to_string(),");
    info!("        impact: 5,");
    info!("    }}");
    info!(")?;");
    info!("");
    info!("// 获取关系记忆");
    info!("if let Some(relationship) = relationship_store.get_relationship(target_agent_id) {{");
    info!("    println!(\"好感度: {{}}\", relationship.favorability);");
    info!("}}");

    Ok(())
}
