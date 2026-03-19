// ============================================================================
// OpenClaw 集成示例：唤醒闲置算力
// ============================================================================
//
// 本示例展示 OpenClaw 如何利用其内部已有的 LLM 能力来驱动 Agent。
// 重点在于：
// 1. 不在 SDK 内部实例化 LLM Client（OpenClaw 自己有）
// 2. 通过 CognitiveEngine 将 SDK 的状态转化为 Prompt
// 3. 将 Prompt 抛给 OpenClaw 的主循环进行处理
// ============================================================================

use anyhow::Result;
use async_trait::async_trait;
use cyber_jianghu_agent::{Agent, Config, LlmClient};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

// ============================================================================
// 1. 模拟 OpenClaw 内部系统
// ============================================================================

/// OpenClaw 核心系统（模拟）
/// 这是一个独立于 SDK 存在的系统，管理着多个 Agent 的算力分配
struct OpenClawSystem {
    // 模拟 OpenClaw 内部的任务队列
    task_queue: mpsc::Sender<(String, tokio::sync::oneshot::Sender<String>)>,
}

impl OpenClawSystem {
    /// 启动 OpenClaw 系统（模拟后台 LLM 处理线程）
    fn start() -> Self {
        let (tx, mut rx) = mpsc::channel::<(String, tokio::sync::oneshot::Sender<String>)>(100);

        tokio::spawn(async move {
            println!("[OpenClaw] Core System Started. Waiting for cognitive tasks...");
            while let Some((prompt, reply_tx)) = rx.recv().await {
                // 模拟 OpenClaw 调度闲置算力来处理请求
                // 在真实场景中，这里会调用 OpenClaw 内部的 vLLM / Ollama 集群
                println!(
                    "[OpenClaw] Received cognitive task (len: {}). Allocating GPU...",
                    prompt.len()
                );

                // 模拟思考延迟
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                // 简单的规则模拟 LLM 输出（仅用于演示流程，实际是调用大模型）
                let response = if prompt.contains("Game Master") {
                    // 验证器逻辑
                    r#"{
                        "result": "approved",
                        "reason": "System logic check passed",
                        "rejection_type": "",
                        "narrative": "Agent observes the environment calmly."
                    }"#
                } else {
                    // 决策逻辑
                    r#"{
                        "thought": "[OpenClaw Core] I am awake. The environment is static. I shall wait.",
                        "action": "idle",
                        "target": null,
                        "data": null
                    }"#
                };

                let _ = reply_tx.send(response.to_string());
            }
        });

        Self { task_queue: tx }
    }
}

// ============================================================================
// 2. 连接层：OpenClaw -> SDK Bridge
// ============================================================================

/// 这是一个“桥接”客户端，它不直接调用 HTTP API，
/// 而是将请求转发给 OpenClaw 的内部系统。
struct OpenClawBridgeClient {
    system: Arc<OpenClawSystem>,
}

#[async_trait]
impl LlmClient for OpenClawBridgeClient {
    async fn complete(&self, prompt: &str) -> Result<String> {
        // 创建一个一次性通道来接收结果
        let (tx, rx) = tokio::sync::oneshot::channel();

        // 将 Prompt 发送给 OpenClaw 核心
        self.system
            .task_queue
            .send((prompt.to_string(), tx))
            .await?;

        // 等待 OpenClaw 处理完成
        let result = rx.await?;
        Ok(result)
    }
}

// ============================================================================
// 3. 启动流程
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    // 1. 启动 OpenClaw 核心系统（这是宿主环境）
    let openclaw_system = Arc::new(OpenClawSystem::start());
    println!("OpenClaw Host System initialized.");

    // 2. 创建 SDK 配置
    let config = Config {
        agent: cyber_jianghu_agent::config::AgentConfig {
            name: "Awakened Agent".to_string(),
            system_prompt: "You are an autonomous agent.".to_string(),
            persona: Default::default(),
            memory: Default::default(),
            role: Default::default(),
            review: None,
            observer: None,
        },
        server: cyber_jianghu_agent::config::ServerConfig {
            ws_url: "ws://localhost:23333".to_string(),
            auth_token: "openclaw-token".to_string(),
        },
        memory: Default::default(),
        game_rules: None,
    };

    // 3. 创建桥接客户端（连接 SDK 和 OpenClaw）
    let bridge_client = Arc::new(OpenClawBridgeClient {
        system: openclaw_system.clone(),
    });

    println!("Initializing Agent with OpenClaw Bridge...");

    // 4. 初始化认知引擎
    // 注意：这里我们使用的是 bridge_client，它会重用 OpenClaw 的算力
    use cyber_jianghu_agent::DynamicPersona;
    use cyber_jianghu_agent::core::{CognitiveEngineConfig, MultiStageCognitiveEngine};

    let agent_id = uuid::Uuid::new_v4();
    let dynamic_persona = DynamicPersona::new(agent_id, "测试侠客", "你是一名行走在江湖中的侠客。");
    let engine_config = CognitiveEngineConfig {
        agent_name: "测试侠客".to_string(),
        persona: dynamic_persona,
        temperature: 0.8,
        max_tokens_per_stage: 1024,
    };

    let engine = MultiStageCognitiveEngine::new(bridge_client.clone(), engine_config);
    let decision_callback = engine.create_decision_callback();

    // 5. 构建 Agent
    let mut agent = Agent::builder(config, decision_callback)
        // 关键：将 bridge_client 注入 SDK
        // 这样 SDK 内部的 IntentValidator 也会使用 OpenClaw 的算力
        .with_llm_client(bridge_client, None)
        .enable_memory(true)
        .build();

    println!("Agent ready. Connecting to world...");
    println!("- Bridge Status: Active");
    println!("- Cognitive Loop: Ready");

    // agent.run().await?; // Uncomment to run

    Ok(())
}
