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
// 5. Web 面板：http://localhost:23340/panel
// 6. HTTP API：http://localhost:23340/api/v1/*
// ============================================================================

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tracing::{Level, debug, error, info, warn};
use uuid::Uuid;
use chrono::Duration;
use reqwest::Client;

use cyber_jianghu_agent::config::{AgentRole, CharacterConfig, Config, IdentityConfig, LlmConfig, RuntimeMode};
use cyber_jianghu_agent::ai::llm::{DirectLlmClient, DirectLlmClientConfig, LlmClient, LlmProvider};
use cyber_jianghu_agent::{
    Agent, AgentBuilder,
    runtime::decision::ws::{WsDecisionState, WsSharedState, DownstreamMessage, run_ws_server},
    runtime::decision::{create_http_state, http_decision},
    runtime::claw::{ClawDecisionState, create_claw_decision_callback},
};
use cyber_jianghu_protocol::{ServerMessage, PendingReview, ReviewDecision, ReviewSubmission};

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
        /// - claw: 等待外部调度器（如 OpenClaw）通过 WebSocket 连接
        /// - cognitive: 内置 LLM 决策，无需外部调度器
        #[arg(long, default_value = "cognitive")]
        mode: String,

        /// Agent 角色
        /// - player: 玩家 Agent，主动决策
        /// - observer: 观察者 Agent，审查玩家意图
        #[arg(long, default_value = "player")]
        role: String,

        /// 观察者模式：目标 Player Agent 的 HTTP 端点
        /// 例如：http://localhost:23340
        #[arg(long, requires = "role")]
        target_endpoint: Option<String>,

        /// 自动创建角色（如果不存在）
        /// 提供角色姓名即可自动创建并注册角色
        #[arg(long)]
        character_name: Option<String>,

        /// 启用观察者 Agent（同时启动 Observer 审查本 Agent）
        /// 注意：仅在 Claw 模式下可用
        #[arg(long)]
        with_observer: bool,
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

    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cyber-jianghu")
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
            config.identity.as_ref().and_then(|i| i.server_url.as_deref()).unwrap_or("(未知)"),
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
fn print_startup_banner(port: u16, server_ws_url: &str, config_path_str: &str) {
    info!("╔══════════════════════════════════════════════╗");
    info!("║       Cyber-Jianghu Agent (Claw Mode)        ║");
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
// 主入口
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Run { port, mode, role, target_endpoint, character_name, with_observer }) => {
            run_agent(port, mode, role, target_endpoint, character_name, with_observer).await?;
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
            run_agent(0, "cognitive".to_string(), "player".to_string(), None, None, false).await?;
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
        println!("  通过 Web 面板创建: http://localhost:23340/panel");
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
            warn!("或通过 Web 面板创建角色: http://localhost:23340/panel");
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
    let provider = LlmProvider::from_str(&llm_config.provider)
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

async fn run_agent(
    port: u16,
    mode: String,
    role: String,
    target_endpoint: Option<String>,
    character_name: Option<String>,
    with_observer: bool,
) -> Result<()> {
    let mut config = load_config()?.unwrap_or_else(|| {
        info!("配置文件不存在，从环境变量加载");
        Config::from_env().unwrap_or_default()
    });
    let config_for_observer = if with_observer { Some(config.clone()) } else { None };

    let runtime_mode = match mode.to_lowercase().as_str() {
        "cognitive" => {
            info!("使用 Cognitive 模式（内置 LLM 决策）");
            RuntimeMode::Cognitive
        }
        "claw" => {
            info!("使用 Claw 模式（等待外部调度器）");
            RuntimeMode::Claw
        }
        _ => {
            info!("未知模式 '{}'，使用 Cognitive 模式", mode);
            RuntimeMode::Cognitive
        }
    };
    config.runtime.mode = runtime_mode;

    // 检查是否为 Observer 模式
    let agent_role = match role.to_lowercase().as_str() {
        "observer" => {
            if target_endpoint.is_none() {
                anyhow::bail!("Observer 模式需要 --target-endpoint 参数指定目标 Player Agent");
            }
            info!("使用 Observer 角色，审查目标: {}", target_endpoint.as_ref().unwrap());
            AgentRole::Observer
        }
        _ => {
            info!("使用 Player 角色");
            AgentRole::Player
        }
    };
    config.role = agent_role;

    ensure_identity(&mut config).await?;

    let identity_clone = config
        .identity
        .as_ref()
        .expect("Identity should exist after ensure_identity")
        .clone();
    let device_id_value = identity_clone.device_id;
    info!("Device ID: {}", device_id_value);

    if let Some(ref name) = character_name {
        if !config.has_character() {
            info!("自动创建角色: {}", name);
            let character = CharacterConfig {
                name: name.clone(),
                ..Default::default()
            };
            let actual_port = if port == 0 { 23340 } else { port };
            match create_character_via_api(actual_port, character).await {
                Ok(agent_id) => {
                    info!("角色创建成功，Agent ID: {}", agent_id);
                    config = load_config()?.unwrap_or_else(|| {
                        warn!("重新加载配置失败，使用内存中的配置");
                        config.clone()
                    });
                }
                Err(e) => {
                    error!("自动创建角色失败: {}", e);
                    error!("请先通过 'cyber-jianghu-agent create-character --name {}' 创建角色", name);
                    return Err(e);
                }
            }
        } else {
            info!("角色已存在: {}", config.agent.as_ref().map(|c| c.name.as_str()).unwrap_or("(未知)"));
        }
    } else if !config.has_character() {
        warn!("尚未创建角色，Agent 将在游戏中处于空闲状态");
        warn!("请通过以下方式创建角色:");
        warn!("  1. Web 面板: http://localhost:23340/panel");
        warn!("  2. CLI: cyber-jianghu-agent create-character --name 名字");
        warn!("  3. 使用 --character-name 参数: cyber-jianghu-agent run --character-name 你的名字");
    }

    // Observer 模式：启动观察者主循环
    if agent_role == AgentRole::Observer {
        let endpoint = target_endpoint.expect("Observer endpoint must be set");
        info!("启动 Observer 模式，审查目标: {}", endpoint);
        run_observer_mode(&config, &endpoint).await?;
        return Ok(());
    }

    let device_id = Arc::new(RwLock::new(device_id_value));

    let persona_info = config.agent.as_ref().map(|c| {
        cyber_jianghu_agent::ai::validator::PersonaInfo {
            gender: c.gender.clone(),
            age: c.age,
            personality: c.personality.clone(),
            values: c.values.clone(),
        }
    });

    // 根据模式创建决策回调和相关组件
    let maybe_callback_setup: Option<ClawCallbackSetup>;
    
    let mut agent = match runtime_mode {
        RuntimeMode::Cognitive => {
            info!("创建 Cognitive 模式组件...");
            let llm_client = create_llm_client(&config.llm)?;
            info!(
                "LLM 配置: provider={}, model={}",
                config.llm.provider,
                config.llm.model.as_deref().unwrap_or("default")
            );
            let claw_state = if let Some(ref character) = config.agent {
                info!("使用角色 persona: {}", character.name);
                ClawDecisionState::new(Arc::new(llm_client))
                    .with_system_prompt(character.generate_system_prompt())
            } else {
                info!("使用默认系统提示词（无角色配置）");
                ClawDecisionState::new(Arc::new(llm_client))
            };
            let decision = create_claw_decision_callback(claw_state);
            maybe_callback_setup = None;
            
            AgentBuilder::new(config, decision).build()
        }
        RuntimeMode::Claw => {
            info!("创建 Claw 模式组件...");
            let setup = start_claw_server(port, device_id.clone(), &config, &identity_clone)?;
            let http_state = setup.http_state.clone();
            maybe_callback_setup = Some(ClawCallbackSetup {
                shared_state: setup.shared_state.clone(),
                api_state: setup.api_state.clone(),
                server_msg_tx: setup.server_msg_tx.clone(),
                device_id: device_id.clone(),
                persona_info: persona_info.clone(),
            });
            let decision = Arc::new(http_decision(
                device_id.clone(),
                http_state,
                55,
            ));
            
            Agent::new(config, decision, Some(setup.reconnect_rx)).await
        }
    };

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
            info!("更新 Claw API device_id: {} -> {}", *old_id, server_agent_id);
            drop(old_id);

            let mut guard = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(device_id_clone.write())
            });
            *guard = server_agent_id;

            if let Some(ref validator) = api_state_clone.intent_validator {
                let mut validator_guard = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(shared_state_clone.intent_validator.write())
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

        agent.set_server_msg_callback(std::sync::Arc::new(move |msg: ServerMessage| {
            let current_tick = 0;
            if let Some(downstream) = DownstreamMessage::from_server_message(msg, current_tick) {
                let _ = server_msg_tx_clone.send(downstream);
            }
        })).await;
    }

    if with_observer && runtime_mode == RuntimeMode::Claw {
        let observer_endpoint = format!("http://localhost:{}", port);
        info!("启动 Observer Agent 审查本 Player...");
        if let Some(observer_config) = config_for_observer {
            tokio::spawn(async move {
                if let Err(e) = run_observer_mode(&observer_config, &observer_endpoint).await {
                    error!("Observer 模式异常退出: {}", e);
                }
            });
        }
    } else if with_observer {
        warn!("--with-observer 仅在 Claw 模式下可用，Cognitive 模式不支持内置 Observer");
    }

    agent.run().await?;
    Ok(())
}

struct ServerSetup {
    reconnect_rx: mpsc::Receiver<cyber_jianghu_agent::runtime::decision::http::ReconnectRequest>,
    server_msg_tx: tokio::sync::broadcast::Sender<DownstreamMessage>,
    shared_state: Arc<WsSharedState>,
    api_state: cyber_jianghu_agent::runtime::decision::http::HttpApiState,
    http_state: Arc<cyber_jianghu_agent::runtime::decision::http::HttpDecisionState>,
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

    info!("启动 Claw 模式（WebSocket + HTTP API），端口: {}", actual_port);

    let config_path_str = config_path().display().to_string();
    print_startup_banner(actual_port, &config.server.ws_url, &config_path_str);

    let (reconnect_tx, reconnect_rx) =
        mpsc::channel::<cyber_jianghu_agent::runtime::decision::http::ReconnectRequest>(10);

    let mut ws_state = WsDecisionState::new();
    let shared_state = Arc::new(WsSharedState::from(&ws_state));
    ws_state.spawn_validation_task((*shared_state).clone());

    let (http_decision_state, api_state) = create_http_state(
        device_id,
        config.server.http_url.clone(),
        config.server.ws_url.clone(),
        Some(identity.clone()),
        Some(reconnect_tx),
        config_path(),
        Some(shared_state.clone()),
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
        http_state: http_decision_state,
    })
}

// ============================================================================
// Observer 模式
// ============================================================================

async fn run_observer_mode(config: &Config, target_endpoint: &str) -> Result<()> {
    info!("Observer 模式启动，审查目标: {}", target_endpoint);
    
    let llm_client = create_llm_client(&config.llm)?;
    info!(
        "Observer LLM 配置: provider={}, model={}",
        config.llm.provider,
        config.llm.model.as_deref().unwrap_or("default")
    );
    
    let client = Client::new();
    let poll_interval = Duration::seconds(5);
    
    loop {
        match fetch_pending_reviews(&client, target_endpoint).await {
            Ok(reviews) if reviews.is_empty() => {
                debug!("暂无待审查意图，{} 秒后再次检查", poll_interval.num_seconds());
            }
            Ok(reviews) => {
                info!("发现 {} 个待审查意图", reviews.len());
                for review in reviews {
                    if let Err(e) = process_review(&client, target_endpoint, &review, &llm_client).await {
                        warn!("审查失败 {}: {}", review.intent_id, e);
                    }
                }
            }
            Err(e) => {
                warn!("获取待审查意图失败: {}，{} 秒后重试", e, poll_interval.num_seconds());
            }
        }
        
        tokio::time::sleep(tokio::time::Duration::from_secs(poll_interval.num_seconds() as u64)).await;
    }
}

async fn fetch_pending_reviews(client: &Client, endpoint: &str) -> Result<Vec<PendingReview>> {
    let url = format!("{}/api/v1/review/pending", endpoint.trim_end_matches('/'));
    let response = client
        .get(&url)
        .send()
        .await
        .context("Failed to fetch pending reviews")?;
    
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to fetch reviews: {} - {}", status, body);
    }
    
    let reviews: Vec<PendingReview> = response
        .json()
        .await
        .context("Failed to parse reviews response")?;
    
    Ok(reviews)
}

async fn process_review(
    client: &Client,
    endpoint: &str,
    review: &PendingReview,
    llm_client: &DirectLlmClient,
) -> Result<()> {
    info!("审查意图 {}: action={}", review.intent_id, review.intent.action_type);
    
    let validation_prompt = format!(
        "审查以下意图是否符合角色人设和世界观规则：\n\n意图: {}\n人设: {:?}\n世界上下文: {}",
        serde_json::to_string(&review.intent)?,
        review.persona_summary,
        review.world_context
    );
    
    let response_text = llm_client.complete(&validation_prompt).await?;
    
    let result = if response_text.to_lowercase().contains("approve") || response_text.to_lowercase().contains("通过") {
        ReviewDecision::Approved
    } else {
        ReviewDecision::Rejected
    };
    
    let submission = ReviewSubmission {
        result,
        reason: response_text,
        narrative: None,
    };
    
    let url = format!("{}/api/v1/review/{}", endpoint.trim_end_matches('/'), review.intent_id);
    let response = client
        .post(&url)
        .json(&submission)
        .send()
        .await
        .context("Failed to submit review")?;
    
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to submit review: {} - {}", status, body);
    }
    
    info!("审查结果已提交: {} -> {:?}", review.intent_id, submission.result);
    Ok(())
}
