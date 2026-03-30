// ============================================================================
// Cyber-Jianghu Agent CLI
// ============================================================================
//
// 连接赛博江湖游戏世界的 Agent CLI
//
// ## 架构说明
//
// Agent 支持两种运行模式：
// - Cognitive 模式（默认）：内置 LLM 决策，Agent 自主做出决策
// - Claw 模式：等待外部 OpenClaw 调度器通过 WebSocket 提交 Intent
//
// ## 使用方式
//
// 1. 首次运行：自动生成 device_id 并向服务器注册
// 2. 后续运行：自动使用已保存的身份连接服务器
// 3. Cognitive 模式（默认）：cyber-jianghu-agent run --mode cognitive
// 4. Claw 模式：cyber-jianghu-agent run --mode claw
// 5. Web 面板：http://localhost:23340/manage.html
// 6. HTTP API：http://localhost:23340/api/v1/*
// ============================================================================

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tracing::{Level, debug, error, info, warn};
use uuid::Uuid;

use cyber_jianghu_agent::ai::llm::{
    DirectLlmClient, DirectLlmClientConfig, LlmClient, LlmProvider,
};
use cyber_jianghu_agent::ai::validator::{
    CognitiveValidator, PersonaInfo, ValidationRequest, ValidationResult, Validator,
};
use cyber_jianghu_agent::config::{
    CharacterConfig, Config, IdentityConfig, LlmConfig, RuntimeMode,
};
use cyber_jianghu_agent::{
    AgentBuilder,
    core::cognitive::{CognitiveEngineConfig, MultiStageCognitiveEngine},
    runtime::claw::{BridgeConfig, OpenClawBridge},
    runtime::decision::create_http_state,
    runtime::decision::http::{
        ConfigWatcher, intent_history::{IntentHistoryStore, ObserverThought},
        review::{PendingReview, ReviewStore}, thinking_log,
    },
    runtime::decision::ws::{DownstreamMessage, WsDecisionState, WsSharedState, run_ws_server},
    runtime::decision::{
        CognitiveDecisionConfig, DecisionCallback, DecisionWithFeedbackCallback,
        cognitive_decision_with_retry_with_chain_store,
    },
};
use cyber_jianghu_protocol::{Intent, ServerMessage, WorldState};

// ============================================================================
// CLI 定义
// ============================================================================

#[derive(Parser)]
#[command(name = "cyber-jianghu-agent")]
#[command(about = "赛博江湖 Agent - 连接游戏世界", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// 运行 Agent（默认命令）
    Run {
        /// 监听端口
        /// 0 = 在 23340~23349 范围内随机选择（推荐，避免与服务器端口 23333 冲突）
        #[arg(short, long, default_value = "0")]
        port: u16,

        /// 运行模式
        ///
        /// - claw: 等待外部调度器（如 OpenClaw）通过 WebSocket 连接
        /// - cognitive: 内置 LLM 决策，无需外部调度器
        ///
        /// 不指定时使用配置文件或环境变量 CYBER_JIANGHU_RUNTIME_MODE 的值
        #[arg(long)]
        mode: Option<String>,
    },

    /// 显示当前配置
    Show,

    /// 配置服务器地址
    Config {
        /// 服务端 WebSocket 地址 (如: ws://localhost:23333/ws)
        #[arg(short, long)]
        ws_url: Option<String>,

        /// 服务端 HTTP 地址 (如: http://localhost:23333)
        #[arg(short, long)]
        http_url: Option<String>,
    },

    /// 创建角色（通过 CLI，也可通过 Web 面板）
    CreateCharacter {
        /// 角色姓名
        #[arg(short, long)]
        name: String,

        /// 角色年龄
        #[arg(long, default_value = "25")]
        age: u8,

        /// 角色性别
        #[arg(long, default_value = "男")]
        gender: String,

        /// 外貌描述
        #[arg(long)]
        appearance: Option<String>,

        /// 身份背景
        #[arg(long)]
        identity: Option<String>,
    },

    /// 重置 Agent 身份（慎用，会清除所有数据）
    Reset,
}

// ============================================================================
// 配置路径
// ============================================================================

fn config_path() -> PathBuf {
    // 支持通过环境变量指定配置目录
    if let Ok(config_dir) = std::env::var("CYBER_JIANGHU_CONFIG_DIR") {
        return PathBuf::from(config_dir).join("agent.yaml");
    }

    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cyber-jianghu")
        .join("config")
        .join("agent.yaml")
}

// ============================================================================
// 配置加载与保存
// ============================================================================

fn load_config() -> Result<Option<Config>> {
    let path = config_path();
    if path.exists() {
        info!("加载配置: {}", path.display());
        let config = Config::from_file(&path).context("Failed to load config")?;
        Ok(Some(config))
    } else {
        Ok(None)
    }
}

fn save_config(config: &Config) -> Result<()> {
    let path = config_path();

    // 创建目录
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    config.save_to_file(&path)?;
    info!("配置已保存到: {}", path.display());
    Ok(())
}

// ============================================================================
// Agent 接入 API
// ============================================================================

#[derive(Debug, Serialize)]
struct AgentConnectRequest {
    device_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct AgentConnectResponse {
    auth_token: String,
    message: String,
}

/// 向服务器注册设备身份
async fn register_agent_identity(server_url: &str, device_id: Uuid) -> Result<String> {
    let client = Client::new();
    let url = format!("{}/api/v1/agent/connect", server_url);

    info!("向服务器注册设备: {} -> {}", device_id, url);

    let response = client
        .post(&url)
        .json(&AgentConnectRequest { device_id })
        .send()
        .await
        .context("Failed to connect to server")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Server returned error {}: {}", status, body);
    }

    let result: AgentConnectResponse = response
        .json()
        .await
        .context("Failed to parse server response")?;

    info!("服务器响应: {}", result.message);
    Ok(result.auth_token)
}

// ============================================================================
// 角色注册 API
// ============================================================================

/// 角色注册响应（从 Agent API 返回）
#[derive(Debug, Deserialize)]
struct CharacterRegisterResponse {
    agent_id: String,
    message: String,
}

/// 通过 Agent API 创建角色
///
/// 将角色配置发送到 Agent HTTP API，由 Agent API 添加设备认证后转发到 Server
async fn create_character_via_api(agent_port: u16, character: CharacterConfig) -> Result<Uuid> {
    let client = Client::new();
    let url = format!("http://localhost:{}/api/v1/character/register", agent_port);

    info!("创建角色: {} -> {}", character.name, url);

    let response = client
        .post(&url)
        .json(&character)
        .send()
        .await
        .context("Failed to create character")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to create character: {} - {}", status, body);
    }

    let result: CharacterRegisterResponse = response
        .json()
        .await
        .context("Failed to parse character response")?;

    info!("角色创建成功: {}", result.message);
    Uuid::parse_str(&result.agent_id).context("Failed to parse agent_id as UUID")
}

// ============================================================================
// 确保 Agent 身份存在
// ============================================================================

async fn ensure_identity(config: &mut Config) -> Result<()> {
    // 检查是否需要重置身份（服务器地址变化）
    let (has_identity, needs_reset) = config.check_identity_server_match();

    if needs_reset {
        warn!(
            "检测到服务器地址变化: {} -> {}",
            config
                .identity
                .as_ref()
                .and_then(|i| i.server_url.as_deref())
                .unwrap_or("(未知)"),
            config.server.http_url
        );
        warn!("将清除旧身份并重新注册...");
        config.clear_identity();
    }

    if has_identity && !needs_reset {
        info!("使用已有 Agent 身份");
        return Ok(());
    }

    info!("首次启动，生成设备身份...");

    // 1. 生成 device_id
    let device_id = Uuid::new_v4();
    info!("生成设备 ID: {}", device_id);

    // 2. 向服务器注册
    let auth_token = register_agent_identity(&config.server.http_url, device_id).await?;

    // 3. 保存身份（包含服务器 URL）
    config.identity = Some(IdentityConfig {
        device_id,
        auth_token,
        server_url: Some(config.server.http_url.clone()),
    });

    // 4. 持久化
    save_config(config)?;
    info!("Agent 身份已创建并保存");

    Ok(())
}

// ============================================================================
// 启动 Banner
// ============================================================================

/// 打印启动 Banner
fn print_startup_banner(port: u16, server_ws_url: &str, config_path_str: &str, mode: &str) {
    let mode_line = format!("Cyber-Jianghu Agent ({})", mode);
    info!("╔══════════════════════════════════════════════╗");
    info!("║{: ^46}║", mode_line);
    info!("╠══════════════════════════════════════════════╣");
    info!("║ HTTP API:  http://0.0.0.0:{}                 ║", port);
    info!("║ WebSocket: {:<34} ║", server_ws_url);
    info!("║ Config:    {:<34} ║", config_path_str);
    info!("╠══════════════════════════════════════════════╣");
    info!("║ 切换服务器: POST /api/v1/config/server       ║");
    info!("║ 热加载配置: POST /api/v1/config/reload       ║");
    info!("║ API 文档:   GET  /api/v1                     ║");
    info!("╚══════════════════════════════════════════════╝");
}

// ============================================================================
// 日志系统初始化
// ============================================================================

fn init_tracing() -> Result<()> {
    let config_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cyber-jianghu");

    let thinking_log_path = thinking_log::init_thinking_log(&config_dir)?;

    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_writer(std::io::stderr)
        .init();

    info!(
        "日志系统已初始化，thinking log: {}",
        thinking_log_path.display()
    );

    Ok(())
}

// ============================================================================
// 主入口
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    init_tracing()?;

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Run { port, mode }) => {
            run_agent(port, mode).await?;
        }

        Some(Commands::Show) => {
            show_config()?;
        }

        Some(Commands::Config { ws_url, http_url }) => {
            update_server_config(ws_url, http_url)?;
        }

        Some(Commands::CreateCharacter {
            name,
            age,
            gender,
            appearance,
            identity,
        }) => {
            create_character_cli(name, age, gender, appearance, identity).await?;
        }

        Some(Commands::Reset) => {
            reset_agent()?;
        }

        None => {
            run_agent(0, None).await?;
        }
    }

    Ok(())
}

// ============================================================================
// 命令实现
// ============================================================================

fn show_config() -> Result<()> {
    let config = load_config()?.unwrap_or_else(|| {
        warn!("配置文件不存在");
        Config::default()
    });

    println!("=== Agent 配置 ===\n");

    if let Some(ref identity) = config.identity {
        println!("Device ID: {}", identity.device_id);
        println!(
            "Auth Token: {}...",
            &identity.auth_token.chars().take(16).collect::<String>()
        );
    } else {
        println!("Device ID: (未注册)");
    }

    println!("\n服务器配置:");
    println!("  WebSocket: {}", config.server.ws_url);
    println!("  HTTP: {}", config.server.http_url);

    if let Some(ref character) = config.agent {
        println!("\n当前角色:");
        println!("  姓名: {}", character.name);
        println!("  年龄: {}", character.age);
        println!("  性别: {}", character.gender);
        if let Some(ref agent_id) = character.agent_id {
            println!("  Agent ID: {}", agent_id);
        } else {
            println!("  Agent ID: (未注册)");
        }
    } else {
        println!("\n当前角色: (未创建)");
        println!("  通过 Web 面板创建: http://localhost:23340/manage.html");
        println!("  或通过 CLI: cyber-jianghu-agent create-character --name 名字");
    }

    println!("\n运行时配置:");
    println!("  模式: {:?}", config.runtime.mode);
    println!("  端口: {}", config.runtime.port);

    Ok(())
}

fn update_server_config(ws_url: Option<String>, http_url: Option<String>) -> Result<()> {
    let mut config = load_config()?.unwrap_or_default();

    if let Some(ws) = ws_url {
        config.server.ws_url = ws;
    }
    if let Some(http) = http_url {
        config.server.http_url = http;
    }

    save_config(&config)?;
    info!("服务器配置已更新");
    Ok(())
}

async fn create_character_cli(
    name: String,
    age: u8,
    gender: String,
    appearance: Option<String>,
    identity: Option<String>,
) -> Result<()> {
    // 检查 Agent 是否已运行
    let port = 23340; // 默认端口

    let character = CharacterConfig {
        name,
        age,
        gender,
        appearance,
        identity,
        ..Default::default()
    };

    match create_character_via_api(port, character).await {
        Ok(agent_id) => {
            info!("角色创建成功! Agent ID: {}", agent_id);
            println!("角色已创建，Agent ID: {}", agent_id);
        }
        Err(e) => {
            warn!("无法连接到 Agent API: {}", e);
            warn!("请确保 Agent 已在 Claw 模式下运行（默认模式）");
            warn!("或通过 Web 面板创建角色: http://localhost:23340/manage.html");
            return Err(e);
        }
    }

    Ok(())
}

fn reset_agent() -> Result<()> {
    let path = config_path();

    warn!("即将删除配置文件: {}", path.display());
    println!("确认删除? (y/N): ");

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    if input.trim().to_lowercase() == "y" {
        if path.exists() {
            std::fs::remove_file(&path)?;
            info!("配置文件已删除");
        }
        println!("Agent 身份已重置，下次启动将生成新的身份");
    } else {
        println!("已取消");
    }

    Ok(())
}

fn create_llm_client(llm_config: &LlmConfig) -> Result<DirectLlmClient> {
    let provider = LlmProvider::parse(&llm_config.provider)
        .ok_or_else(|| anyhow::anyhow!("Unknown LLM provider: {}", llm_config.provider))?;

    let mut client_config = DirectLlmClientConfig::new(provider, llm_config.api_key.clone());

    if let Some(url) = &llm_config.base_url {
        client_config = client_config.with_base_url(url);
    }
    if let Some(model) = &llm_config.model {
        client_config = client_config.with_model(model);
    }
    client_config = client_config
        .with_temperature(llm_config.temperature)
        .with_max_tokens(llm_config.max_tokens);

    DirectLlmClient::new(client_config)
}

// ============================================================================
// 运行 Agent
// ============================================================================

async fn run_agent(port: u16, mode: Option<String>) -> Result<()> {
    let mut config = load_config()?.unwrap_or_else(|| {
        info!("配置文件不存在，从环境变量加载");
        Config::from_env().unwrap_or_default()
    });

    // CLI 参数优先，否则使用配置文件/环境变量
    if let Some(mode_str) = mode {
        let runtime_mode = match mode_str.to_lowercase().as_str() {
            "cognitive" => {
                info!("使用 Cognitive 模式（内置 LLM 决策）");
                RuntimeMode::Cognitive
            }
            "claw" => {
                info!("使用 Claw 模式（等待外部调度器）");
                RuntimeMode::Claw
            }
            _ => {
                info!("未知模式 '{}'，使用配置文件中的设置", mode_str);
                // 不覆盖，保持配置文件值
                config.runtime.mode
            }
        };
        if runtime_mode != config.runtime.mode {
            config.runtime.mode = runtime_mode;
        }
    } else {
        match config.runtime.mode {
            RuntimeMode::Cognitive => {
                info!("使用 Cognitive 模式（内置 LLM 决策）");
            }
            RuntimeMode::Claw => {
                info!("使用 Claw 模式（等待外部调度器）");
            }
        }
    }

    // 设置配置文件路径（用于热重载）
    let config_path = config_path();
    config.config_path = config_path.clone();

    ensure_identity(&mut config).await?;

    let identity_clone = config
        .identity
        .as_ref()
        .expect("Identity should exist after ensure_identity")
        .clone();
    let device_id_value = identity_clone.device_id;
    info!("Device ID: {}", device_id_value);

    if !config.has_character() {
        warn!("尚未创建角色，Agent 将在游戏中处于空闲状态");
        warn!("请通过以下方式创建角色:");
        warn!("  1. Web 面板: http://localhost:23340/manage.html");
        warn!("  2. CLI: cyber-jianghu-agent create-character --name 名字");
    }

    let device_id = Arc::new(RwLock::new(device_id_value));

    let persona_info =
        config
            .agent
            .as_ref()
            .map(|c| cyber_jianghu_agent::ai::validator::PersonaInfo {
                gender: c.gender.clone(),
                age: c.age,
                personality: c.personality.clone(),
                values: c.values.clone(),
            });

    // 根据模式创建决策回调和相关组件
    let maybe_callback_setup: Option<ClawCallbackSetup>;
    let cognitive_death_event_tx: Option<tokio::sync::broadcast::Sender<ServerMessage>>;
    let cognitive_api_state: Option<
        Arc<cyber_jianghu_agent::runtime::decision::http::HttpApiState>,
    >;

    let mut agent = match config.runtime.mode {
        RuntimeMode::Cognitive => {
            info!("创建 Cognitive 模式组件...");
            let llm_client = create_llm_client(&config.llm)?;
            info!(
                "LLM 配置: provider={}, model={}",
                config.llm.provider,
                config.llm.model.as_deref().unwrap_or("default")
            );

            let llm_arc: Arc<dyn LlmClient> = Arc::new(llm_client);
            let llm_container = Arc::new(RwLock::new(llm_arc.clone()));
            info!("LLM Client 容器已创建（支持热重载）");

            let agent_name = config
                .agent
                .as_ref()
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "(未创建)".to_string());
            let agent_id = config
                .identity
                .as_ref()
                .map(|i| i.device_id)
                .unwrap_or_else(Uuid::new_v4);

            let persona_description = config
                .agent
                .as_ref()
                .map(|c| c.generate_system_prompt())
                .unwrap_or_else(|| "你是一名行走在江湖中的侠客。".to_string());

            let cognitive_config = CognitiveEngineConfig {
                agent_name: agent_name.to_string(),
                persona: cyber_jianghu_agent::ai::persona::DynamicPersona::new(
                    agent_id,
                    &agent_name,
                    &persona_description,
                ),
                temperature: config.llm.temperature,
                max_tokens_per_stage: config.llm.max_tokens,
            };
            let cognitive_engine = Arc::new(MultiStageCognitiveEngine::new(
                llm_arc.clone(),
                cognitive_config,
            ));
            let llm_enabled = cognitive_engine.llm_enabled_handle();

            let last_cognitive_chain = Arc::new(tokio::sync::RwLock::new(None::<cyber_jianghu_agent::core::cognitive::CognitiveChain>));
            let cognitive_decision_with_feedback: DecisionWithFeedbackCallback =
                Arc::new(cognitive_decision_with_retry_with_chain_store(
                    agent_id,
                    cognitive_engine.clone(),
                    CognitiveDecisionConfig::default().max_retries,
                    Some(last_cognitive_chain.clone()),
                ));

            let cognitive_engine_for_builder = cognitive_engine.clone();
            let decision: DecisionCallback = Arc::new(move |ws: &WorldState| {
                let engine = cognitive_engine.clone();
                let ws = ws.clone();
                Box::pin(async move {
                    match engine.think(&ws).await {
                        Ok(chain) => chain.final_intent,
                        Err(e) => {
                            error!("[cognitive] Decision failed: {}", e);
                            Intent::new(Uuid::nil(), ws.tick_id, "idle", None)
                                .with_thought(format!("认知失败: {}", e))
                        }
                    }
                })
            });

            let (reconnect_tx, reconnect_rx) =
                mpsc::channel::<cyber_jianghu_agent::runtime::decision::http::ReconnectRequest>(10);

            let (api_state, actual_port) = start_http_api_server(
                port,
                device_id.clone(),
                &config,
                Some(reconnect_tx),
                Some(llm_enabled),
            )?;
            info!("HTTP API 已启动: http://localhost:{}", actual_port);
            info!("Web 面板: http://localhost:{}/", actual_port);
            info!("角色管理: http://localhost:{}/index.html", actual_port);

            let browser_url = format!("http://localhost:{}/welcome.html", actual_port);
            let is_container = std::path::Path::new("/app/.dockerenv").exists();
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                if is_container {
                    debug!("浏览器请手动打开: {}", browser_url);
                } else {
                    match open::that_detached(&browser_url) {
                        Ok(_) => info!("浏览器已打开: {}", browser_url),
                        Err(e) => debug!("无法自动打开浏览器: {}，请手动访问: {}", e, browser_url),
                    }
                }
            });

            let review_store =
                Arc::new(ReviewStore::new(config.review.clone().unwrap_or_default()));
            info!("ReviewStore 已创建（ReflectorSoul 默认启用）");

            let config_watcher = Arc::new(ConfigWatcher::new(config_path.clone())?);
            info!("ConfigWatcher 已创建，支持 LLM 配置热重载");

            let reflector_config_watcher = config_watcher.clone();

            let agent = AgentBuilder::new(config, decision)
                .with_decision_feedback(cognitive_decision_with_feedback)
                .with_review_store(review_store.clone())
                .with_llm_client(llm_arc.clone(), None)
                .with_llm_container(llm_container)
                .with_cognitive_engine(cognitive_engine_for_builder)
                .with_cognitive_validator(Arc::new(CognitiveValidator::new(
                    agent_name.to_string(),
                )))
                .with_last_cognitive_chain_store(last_cognitive_chain.clone())
                .with_config_reload_rx(config_watcher.subscribe())
                .with_http_api_state(api_state.clone())
                .with_reconnect_rx(reconnect_rx)
                .build();
            let intent_history = api_state.intent_history.clone();
            let validator = agent.validator().unwrap();
            tokio::spawn(async move {
                if let Err(e) = run_reflector_soul_task(
                    reflector_config_watcher.subscribe(),
                    review_store,
                    intent_history,
                    validator,
                )
                .await
                {
                    error!("ReflectorSoul 任务异常退出: {}", e);
                }
            });
            info!("ReflectorSoul 任务已启动（反思之魂）");

            maybe_callback_setup = None;
            cognitive_death_event_tx = Some(api_state.death_event_tx.clone());
            cognitive_api_state = Some(api_state.clone());
            agent
        }
        RuntimeMode::Claw => {
            info!("创建 Claw 模式组件...");
            let setup = start_claw_server(port, device_id.clone(), &config, &identity_clone)?;
            cognitive_death_event_tx = None;
            cognitive_api_state = None;
            maybe_callback_setup = Some(ClawCallbackSetup {
                shared_state: setup.shared_state.clone(),
                api_state: setup.api_state.clone(),
                server_msg_tx: setup.server_msg_tx.clone(),
                device_id: device_id.clone(),
                persona_info: persona_info.clone(),
            });

            // 创建 ReviewStore（ActorSoul + ReflectorSoul 共享）
            let review_store =
                Arc::new(ReviewStore::new(config.review.clone().unwrap_or_default()));
            info!("ReviewStore 已创建（ReflectorSoul 默认启用）");

            // === 统一认知架构路径 ===
            // 获取 LLM 通信通道
            let upstream_tx = setup.shared_state.upstream_tx.clone();
            let llm_response_rx = setup.shared_state.llm_response_rx.lock().unwrap().take();

            // 创建 OpenClawBridge 作为 LlmClient 实现
            let openclaw_bridge =
                Arc::new(OpenClawBridge::new(upstream_tx, BridgeConfig::default()));
            info!("OpenClawBridge 已创建（Claw 模式 LLM 客户端）");

            // 启动 LLM 响应转发任务
            if let Some(mut response_rx) = llm_response_rx {
                let bridge = openclaw_bridge.clone();
                tokio::spawn(async move {
                    while let Some((request_id, result)) = response_rx.recv().await {
                        bridge.handle_response(
                            &request_id,
                            result.map_err(|e| anyhow::anyhow!("{}", e)),
                        );
                    }
                    info!("LLM 响应转发任务结束");
                });
                info!("LLM 响应转发任务已启动");
            }

            // 创建 MultiStageCognitiveEngine（与 Cognitive 模式共享架构）
            let agent_name = config
                .agent
                .as_ref()
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "(未创建)".to_string());
            let agent_id = config
                .identity
                .as_ref()
                .map(|i| i.device_id)
                .unwrap_or_else(Uuid::new_v4);
            let persona_description = config
                .agent
                .as_ref()
                .map(|c| c.generate_system_prompt())
                .unwrap_or_else(|| "你是一名行走在江湖中的侠客。".to_string());

            let cognitive_config = CognitiveEngineConfig {
                agent_name: agent_name.clone(),
                persona: cyber_jianghu_agent::ai::persona::DynamicPersona::new(
                    agent_id,
                    &agent_name,
                    &persona_description,
                ),
                temperature: config.llm.temperature,
                max_tokens_per_stage: config.llm.max_tokens,
            };

            let llm_client: Arc<dyn LlmClient> = openclaw_bridge;
            let cognitive_engine = Arc::new(MultiStageCognitiveEngine::new(
                llm_client.clone(),
                cognitive_config,
            ));
            info!("MultiStageCognitiveEngine 已创建（Claw 模式统一认知架构）");

            let last_cognitive_chain = Arc::new(tokio::sync::RwLock::new(None::<cyber_jianghu_agent::core::cognitive::CognitiveChain>));
            let cognitive_decision_with_feedback: DecisionWithFeedbackCallback =
                Arc::new(cognitive_decision_with_retry_with_chain_store(
                    agent_id,
                    cognitive_engine.clone(),
                    CognitiveDecisionConfig::default().max_retries,
                    Some(last_cognitive_chain.clone()),
                ));

            let cognitive_engine_for_builder = cognitive_engine.clone();
            let decision: DecisionCallback = Arc::new(move |ws: &WorldState| {
                let engine = cognitive_engine.clone();
                let ws = ws.clone();
                Box::pin(async move {
                    match engine.think(&ws).await {
                        Ok(chain) => chain.final_intent,
                        Err(e) => {
                            error!("[claw-cognitive] Decision failed: {}", e);
                            Intent::new(Uuid::nil(), ws.tick_id, "idle", None)
                                .with_thought(format!("认知失败: {}", e))
                        }
                    }
                })
            });

            // 启动 ReflectorSoul 任务（Claw 模式无配置热重载）
            let (_reflector_config_reload_tx, _reflector_config_reload_rx) =
                tokio::sync::broadcast::channel::<()>(1);

            // 使用 AgentBuilder 与 Cognitive 模式保持一致（COI 原则）
            let agent = AgentBuilder::new(config, decision)
                .with_decision_feedback(cognitive_decision_with_feedback)
                .with_reconnect_rx(setup.reconnect_rx)
                .with_review_store(review_store.clone())
                .with_llm_client(llm_client.clone(), None)
                .with_cognitive_engine(cognitive_engine_for_builder)
                .with_cognitive_validator(Arc::new(CognitiveValidator::new(
                    agent_name.to_string(),
                )))
                .with_last_cognitive_chain_store(last_cognitive_chain.clone())
                .with_http_api_state(Arc::new(setup.api_state.clone()))
                .build();

            let intent_history = setup.api_state.intent_history.clone();
            let validator = agent.validator().unwrap();
            tokio::spawn(async move {
                if let Err(e) = run_reflector_soul_task(
                    tokio::sync::broadcast::channel::<()>(1).1,
                    review_store,
                    intent_history,
                    validator,
                )
                .await
                {
                    error!("ReflectorSoul 任务异常退出: {}", e);
                }
            });
            info!("ReflectorSoul 任务已启动（反思之魂）");

            agent
        }
    };

    // Cognitive 模式：设置死亡事件回调
    if let (Some(death_tx), Some(api_state)) = (cognitive_death_event_tx, cognitive_api_state) {
        let death_tx_clone = death_tx.clone();
        let api_state_clone = api_state.clone();
        agent
            .set_server_msg_callback(std::sync::Arc::new(move |msg: ServerMessage| {
                if matches!(msg, ServerMessage::AgentDied { .. }) {
                    api_state_clone
                        .is_dead
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    let _ = death_tx_clone.send(msg);
                }
            }))
            .await;
    }

    if let Some(setup) = maybe_callback_setup {
        let shared_state_clone = setup.shared_state.clone();
        let api_state_clone = setup.api_state.clone();
        let server_msg_tx_clone = setup.server_msg_tx.clone();
        let device_id_clone = setup.device_id.clone();
        let persona_clone = setup.persona_info.clone();

        agent.set_registration_callback(std::sync::Arc::new(move |server_agent_id: Uuid| {
            let old_id = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(device_id_clone.read())
            });
            info!(
                "更新 Claw API device_id: {} -> {}",
                *old_id, server_agent_id
            );
            drop(old_id);

            let mut guard = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(device_id_clone.write())
            });
            *guard = server_agent_id;

            if let Some(ref validator) = api_state_clone.intent_validator {
                let mut validator_guard = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current()
                        .block_on(shared_state_clone.intent_validator.write())
                });
                *validator_guard = Some(validator.clone());
                info!("Validator injected into WsSharedState");
            }

            if let Some(ref persona) = persona_clone {
                let mut persona_guard = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(shared_state_clone.persona.write())
                });
                *persona_guard = Some(persona.clone());
                info!("Persona injected into WsSharedState");
            }
        }));

        agent
            .set_server_msg_callback(std::sync::Arc::new(move |msg: ServerMessage| {
                if matches!(msg, ServerMessage::AgentDied { .. }) {
                    api_state_clone
                        .is_dead
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    let _ = api_state_clone.death_event_tx.send(msg.clone());
                }
                let current_tick = 0;
                if let Some(downstream) = DownstreamMessage::from_server_message(msg, current_tick)
                {
                    let _ = server_msg_tx_clone.send(downstream);
                }
            }))
            .await;
    }

    agent.run().await?;
    Ok(())
}

struct ServerSetup {
    reconnect_rx: mpsc::Receiver<cyber_jianghu_agent::runtime::decision::http::ReconnectRequest>,
    server_msg_tx: tokio::sync::broadcast::Sender<DownstreamMessage>,
    shared_state: Arc<WsSharedState>,
    api_state: cyber_jianghu_agent::runtime::decision::http::HttpApiState,
}

struct ClawCallbackSetup {
    shared_state: Arc<WsSharedState>,
    api_state: cyber_jianghu_agent::runtime::decision::http::HttpApiState,
    server_msg_tx: tokio::sync::broadcast::Sender<DownstreamMessage>,
    device_id: Arc<RwLock<Uuid>>,
    persona_info: Option<cyber_jianghu_agent::ai::validator::PersonaInfo>,
}

fn start_claw_server(
    port: u16,
    device_id: Arc<RwLock<Uuid>>,
    config: &Config,
    identity: &IdentityConfig,
) -> Result<ServerSetup> {
    let actual_port = if port == 0 {
        use rand::RngExt;
        let random_port = rand::rng().random_range(23340..=23349);
        info!("随机选择端口: {} (范围: 23340-23349)", random_port);
        random_port
    } else {
        port
    };

    info!(
        "启动 Claw 模式（WebSocket + HTTP API），端口: {}",
        actual_port
    );

    let config_path_str = config_path().display().to_string();
    print_startup_banner(actual_port, &config.server.ws_url, &config_path_str, "Claw");

    let (reconnect_tx, reconnect_rx) =
        mpsc::channel::<cyber_jianghu_agent::runtime::decision::http::ReconnectRequest>(10);

    let mut ws_state = WsDecisionState::new();
    let shared_state = Arc::new(WsSharedState::from(&ws_state));
    ws_state.spawn_validation_task((*shared_state).clone());

    let (_http_decision_state, api_state) = create_http_state(
        device_id,
        config.server.http_url.clone(),
        config.server.ws_url.clone(),
        Some(identity.clone()),
        Some(reconnect_tx),
        config_path(),
        Some(shared_state.clone()),
        config.runtime.mode,
        None,
    );

    let api_state_clone = api_state.clone();
    let server_msg_tx = shared_state.server_msg_tx.clone();
    let shared_state_for_callback = shared_state.clone();
    let api_state_for_callback = api_state.clone();
    tokio::spawn(async move {
        if let Err(e) = run_ws_server(actual_port, (*shared_state).clone(), api_state_clone).await {
            error!("Claw server error: {}", e);
        }
    });

    Ok(ServerSetup {
        reconnect_rx,
        server_msg_tx,
        shared_state: shared_state_for_callback,
        api_state: api_state_for_callback,
    })
}

fn start_http_api_server(
    port: u16,
    device_id: Arc<RwLock<Uuid>>,
    config: &Config,
    reconnect_tx: Option<
        mpsc::Sender<cyber_jianghu_agent::runtime::decision::http::ReconnectRequest>,
    >,
    llm_enabled: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> Result<(
    Arc<cyber_jianghu_agent::runtime::decision::http::HttpApiState>,
    u16,
)> {
    let port_range_start = 23340u16;
    let port_range_end = 23349u16;

    let actual_port = if port == 0 {
        use rand::RngExt;
        let random_port = rand::rng().random_range(port_range_start..=port_range_end);
        info!(
            "随机选择 HTTP API 端口: {} (范围: {}-{})",
            random_port, port_range_start, port_range_end
        );
        random_port
    } else {
        port
    };

    info!("启动 HTTP API 服务器，端口: {}", actual_port);

    let config_path_str = config_path().display().to_string();
    print_startup_banner(
        actual_port,
        &config.server.ws_url,
        &config_path_str,
        "Cognitive",
    );

    let (_http_decision_state, api_state) =
        cyber_jianghu_agent::runtime::decision::create_http_state(
            device_id,
            config.server.http_url.clone(),
            config.server.ws_url.clone(),
            config.identity.clone(),
            reconnect_tx,
            config_path(),
            None,
            config.runtime.mode,
            llm_enabled,
        );

    let api_state_clone = api_state.clone();
    let is_auto_port = port == 0;
    let resolved_port = Arc::new(tokio::sync::Mutex::new(actual_port));
    let resolved_port_clone = resolved_port.clone();
    tokio::spawn(async move {
        let mut try_port = actual_port;

        loop {
            match cyber_jianghu_agent::runtime::decision::run_http_server(
                try_port,
                api_state_clone.clone(),
            )
            .await
            {
                Ok(()) => return,
                Err(e) if is_auto_port => {
                    let mut next = try_port + 1;
                    if next > port_range_end {
                        next = port_range_start;
                    }
                    if next == actual_port {
                        error!(
                            "HTTP API server error: 所有端口 {}-{} 均被占用: {}",
                            port_range_start, port_range_end, e
                        );
                        return;
                    }
                    warn!(
                        "HTTP API server error: 端口 {} 被占用 ({})，尝试端口 {}",
                        try_port, e, next
                    );
                    *resolved_port_clone.lock().await = next;
                    try_port = next;
                }
                Err(e) => {
                    error!("HTTP API server error: {}", e);
                    return;
                }
            }
        }
    });

    let final_port = actual_port;
    Ok((Arc::new(api_state), final_port))
}

// ============================================================================
// ReflectorSoul (反思之魂)
// ============================================================================

/// 运行 ReflectorSoul 任务（默认启用）
///
/// ReflectorSoul 作为反思之魂（超我），审查 ActorSoul 生成的意图
/// 通过 ReviewStore 共享内存进行通信（进程内双 Soul 架构）
async fn run_reflector_soul_task(
    mut config_reload_rx: tokio::sync::broadcast::Receiver<()>,
    review_store: Arc<ReviewStore>,
    intent_history: Option<
        Arc<cyber_jianghu_agent::runtime::decision::http::intent_history::IntentHistoryStore>,
    >,
    validator: Arc<dyn cyber_jianghu_agent::ai::validator::Validator>,
) -> Result<()> {
    info!("ReflectorSoul 启动（反思之魂），审查 ActorSoul 意图");

    let review_notify = review_store.notify();
    let fallback_interval = tokio::time::Duration::from_secs(30);

    loop {
        tokio::select! {
            // 配置变更通知（与 ActorSoul 同步热重载）
            Ok(()) = config_reload_rx.recv() => {
                info!("ReflectorSoul 检测到配置变更...");
            }
            // ReviewStore 通知唤醒（即时响应 ActorSoul 提交）
            _ = review_notify.notified() => {
                // 处理所有 pending（可能连续多个）
                loop {
                    let reviews = review_store.get_pending().await;
                    if reviews.is_empty() {
                        break;
                    }
                    info!("[ReflectorSoul] 发现 {} 个待审查意图", reviews.len());
                    for review in reviews {
                        if let Err(e) = process_review_with_store(
                            &review_store,
                            &review,
                            intent_history.as_ref(),
                            &validator,
                        )
                        .await
                        {
                            warn!("[ReflectorSoul] 审查失败 {}: {}", review.intent_id, e);
                        }
                    }
                }
            }
            // 兜底轮询（防止 notify 遗漏）
            _ = tokio::time::sleep(fallback_interval) => {
                let reviews = review_store.get_pending().await;
                if !reviews.is_empty() {
                    info!("[ReflectorSoul] 兜底轮询发现 {} 个待审查意图", reviews.len());
                    for review in reviews {
                        if let Err(e) = process_review_with_store(
                            &review_store,
                            &review,
                            intent_history.as_ref(),
                            &validator,
                        )
                        .await
                        {
                            warn!("[ReflectorSoul] 审查失败 {}: {}", review.intent_id, e);
                        }
                    }
                }
            }
        }
    }
}

/// 处理单个审查请求（ReflectorSoul）
async fn process_review_with_store(
    review_store: &Arc<ReviewStore>,
    review: &PendingReview,
    intent_history: Option<&Arc<IntentHistoryStore>>,
    validator: &Arc<dyn Validator>,
) -> Result<()> {
    use cyber_jianghu_agent::runtime::decision::http::review::ReviewDecision;
    use cyber_jianghu_protocol::ReviewSubmission as ProtocolReviewSubmission;

    info!(
        "[ReflectorSoul] 审查意图 {}: action={}",
        review.intent_id, review.intent.action_type
    );

    // PersonaSummary → PersonaInfo 映射（丢弃 name 字段）
    let persona = PersonaInfo {
        gender: review.persona_summary.gender.clone(),
        age: review.persona_summary.age,
        personality: review.persona_summary.personality.clone(),
        values: review.persona_summary.values.clone(),
    };

    let request = ValidationRequest {
        intent: review.intent.clone(),
        persona,
        world_context: review.world_context.clone(),
    };

    let (result, reason_text, narrative_text) = match validator.validate(request).await {
        Ok(ValidationResult::Approved { reason, narrative }) => {
            (ReviewDecision::Approved, reason.unwrap_or_default(), narrative)
        }
        Ok(ValidationResult::Rejected { reason, .. }) => {
            (ReviewDecision::Rejected, reason, String::new())
        }
        Err(e) => {
            // 验证失败默认为 Rejected（安全优先）
            warn!("[ReflectorSoul] 验证失败，默认 Rejected: {}", e);
            (
                ReviewDecision::Rejected,
                format!("验证系统异常: {}", e),
                String::new(),
            )
        }
    };

    let submission = ProtocolReviewSubmission {
        result,
        reason: reason_text.clone(),
        narrative: if narrative_text.is_empty() {
            None
        } else {
            Some(narrative_text.clone())
        },
    };

    review_store
        .submit_review(review.intent_id, submission)
        .await
        .map_err(|e| anyhow::anyhow!("ReflectorSoul submit_review failed: {:?}", e))?;

    // 更新经历日志中的 observer_thought（供 Web Panel 查询）
    if let Some(history) = intent_history {
        let observer_thought = ObserverThought {
            result: format!("{:?}", result).to_lowercase(),
            reason: reason_text.clone(),
            narrative: Some(narrative_text.clone()),
        };
        history
            .update_observer_thought(review.intent.tick_id, observer_thought)
            .await;
        info!(
            "[ReflectorSoul] Updated observer thought for tick {} in intent_history",
            review.intent.tick_id
        );
    }

    info!(
        "[ReflectorSoul] 审查结果已提交: {} -> {:?}",
        review.intent_id, result
    );
    Ok(())
}
