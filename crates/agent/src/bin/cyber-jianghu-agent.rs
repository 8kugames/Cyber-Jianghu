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
// 5. Web 面板：http://localhost:<端口>/welcome.html
// 6. HTTP API：http://localhost:<端口>/api/v1/*
// ============================================================================

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use notify::{self, Watcher};
use reqwest::Client;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{Level, debug, error, info, warn};
use uuid::Uuid;

use cyber_jianghu_agent::component::llm::{
    DirectLlmClient, DirectLlmClientConfig, FallbackLlmClient, LlmClient, LlmProvider,
};
use cyber_jianghu_agent::config::{
    CharacterConfig, CharacterStatus, Config, DeviceConfig, LlmConfig, RuntimeMode,
};
use cyber_jianghu_agent::{
    AgentBuilder,
    infra::api::thinking_log,
    runtime::claw::{BridgeConfig, OpenClawBridge},
    runtime::claw::{DownstreamMessage, WsDecisionState, WsSharedState, run_ws_server},
    runtime::create_http_state,
    runtime::{
        CognitiveDecisionConfig, DecisionCallback, DecisionWithFeedbackCallback,
        cognitive_decision_with_retry,
    },
    soul::actor::{CognitiveEngine, CognitiveEngineConfig},
    soul::translator::IntentTranslator,
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
        /// - claw: 等待外部调度器（如 OpenClaw）通过 WebSocket 连接
        /// - cognitive: 内置 LLM 决策，无需外部调度器
        #[arg(long, default_value = "cognitive")]
        mode: String,

        /// Server WebSocket URL (overrides agent.yaml)
        #[arg(long)]
        server: Option<String>,
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
// 确保设备身份存在（server-scoped）
// ============================================================================

async fn ensure_device(config: &Config, ws_url: &str) -> Result<DeviceConfig> {
    let device_path = config.device_yaml_path(ws_url);

    if device_path.exists() {
        let device = DeviceConfig::from_file(&device_path)?;
        info!("使用已有设备身份: {}", device.device_id);
        return Ok(device);
    }

    info!("首次启动，生成设备身份...");

    // 1. 生成 device_id
    let device_id = Uuid::new_v4();
    info!("生成设备 ID: {}", device_id);

    // 2. Derive HTTP URL from WS URL
    let http_url = cyber_jianghu_agent::config::ws_to_http_url(ws_url);

    // 3. 向服务器注册
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/v1/agent/connect", http_url))
        .json(&serde_json::json!({"device_id": device_id.to_string()}))
        .send()
        .await
        .context("Failed to register device with server")?;

    let body: serde_json::Value = resp
        .json()
        .await
        .context("Failed to parse device registration response")?;
    let auth_token = body["auth_token"]
        .as_str()
        .context("No auth_token in response")?
        .to_string();

    // 4. 创建 DeviceConfig
    let device = DeviceConfig {
        device_id,
        auth_token,
        server_url: ws_url.to_string(),
    };

    // 5. 持久化
    if let Some(parent) = device_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    device.save_to_file(&device_path)?;

    info!("设备身份已创建并保存: {} (server: {})", device_id, ws_url);
    Ok(device)
}

// ============================================================================
// 选择角色（从 filesystem）
// ============================================================================

fn select_character(server_dir: &Path) -> Option<CharacterConfig> {
    let chars_dir = server_dir.join("characters");
    if !chars_dir.exists() {
        return None;
    }

    let mut alive: Vec<CharacterConfig> = vec![];
    if let Ok(entries) = std::fs::read_dir(&chars_dir) {
        for entry in entries.flatten() {
            if !entry.file_type().ok()?.is_dir() {
                continue;
            }
            let path = entry.path().join("character.yaml");
            if let Ok(config) = CharacterConfig::from_file(&path)
                && config.status == CharacterStatus::Alive
            {
                alive.push(config);
            }
        }
    }

    alive.into_iter().next()
}

// ============================================================================
// 启动 Banner
// ============================================================================

/// 打印启动 Banner
fn print_startup_banner(port: u16, server_ws_url: &str, config_path_str: &str, mode: &str) {
    info!("╔══════════════════════════════════════════════╗");
    info!("║   Cyber-Jianghu Agent ({:^20})   ║", mode);
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
        Some(Commands::Run { port, mode, server }) => {
            run_agent(port, mode, server).await?;
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
            run_agent(0, "cognitive".to_string(), None).await?;
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

    println!("服务器配置:");
    println!("  WebSocket: {}", config.server.ws_url);
    println!("  HTTP: {}", config.server.http_url);

    // Show device status for default server
    let server_dir = config.server_dir(&config.server.ws_url);
    let device_path = config.device_yaml_path(&config.server.ws_url);
    if device_path.exists() {
        if let Ok(device) = DeviceConfig::from_file(&device_path) {
            println!("\n设备身份:");
            println!("  Device ID: {}", device.device_id);
            println!(
                "  Auth Token: {}...",
                &device.auth_token.chars().take(16).collect::<String>()
            );
        }
    } else {
        println!("\n设备身份: (未注册)");
    }

    // Show characters for this server
    if let Some(character) = select_character(&server_dir) {
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
        let display_port = if config.runtime.port == 0 {
            "<自动>".to_string()
        } else {
            config.runtime.port.to_string()
        };
        println!(
            "  通过 Web 面板创建: http://localhost:{}/welcome.html",
            display_port
        );
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
            warn!(
                "或通过 Web 面板创建角色: http://localhost:{}/welcome.html",
                port
            );
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

/// 创建指定 model name 的 LLM 客户端（用于 fallback）
fn create_llm_client_with_model(llm_config: &LlmConfig, model: &str) -> Result<DirectLlmClient> {
    let provider = LlmProvider::parse(&llm_config.provider)
        .ok_or_else(|| anyhow::anyhow!("Unknown LLM provider: {}", llm_config.provider))?;

    let mut client_config = DirectLlmClientConfig::new(provider, llm_config.api_key.clone());

    if let Some(url) = &llm_config.base_url {
        client_config = client_config.with_base_url(url);
    }
    client_config = client_config
        .with_model(model)
        .with_temperature(llm_config.temperature)
        .with_max_tokens(llm_config.max_tokens);

    DirectLlmClient::new(client_config)
}

// ============================================================================
// 等待角色创建
// ============================================================================

/// Waits for a valid character to appear in the characters directory.
/// HTTP API must be started before calling this function.
async fn await_character_loop(server_dir: &Path) -> Result<()> {
    let characters_dir = server_dir.join("characters");

    std::fs::create_dir_all(&characters_dir).context("Failed to create characters directory")?;

    info!("Waiting for character creation...");
    info!("Access web panel to create a character");

    // Try notify first, fallback to polling
    let mut watcher = match notify::recommended_watcher(|_| {}) {
        Ok(w) => Some(w),
        Err(e) => {
            warn!("notify unavailable, using polling fallback: {}", e);
            None
        }
    };

    if let Some(ref mut w) = watcher {
        w.watch(&characters_dir, notify::RecursiveMode::NonRecursive)
            .ok();
    }

    loop {
        if let Some(c) = select_character(server_dir)
            && c.agent_id.is_some()
            && c.status == CharacterStatus::Alive
        {
            info!("Character found: {} ({})", c.name, c.agent_id.unwrap());
            return Ok(());
        }

        if watcher.is_none() {
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        } else {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    }
}

// ============================================================================
// 运行 Agent
// ============================================================================

async fn run_agent(port: u16, mode: String, server: Option<String>) -> Result<()> {
    let mut config = load_config()?.unwrap_or_else(|| {
        info!("配置文件不存在，从环境变量加载");
        Config::from_env().unwrap_or_default()
    });

    // Ensure servers_dir is set (#[serde(skip)] means it's empty after from_file)
    if config.servers_dir.as_os_str().is_empty() {
        config.servers_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".cyber-jianghu")
            .join("servers");
    }

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

    // Determine server URL (CLI arg overrides config)
    let ws_url = server.as_deref().unwrap_or(&config.server.ws_url);
    info!("连接服务器: {}", ws_url);

    // Set config path for hot reload
    let config_path = config_path();
    let mut config_for_builder = config.clone();
    config_for_builder.config_path = config_path.clone();
    config_for_builder.runtime.mode = runtime_mode;

    // Ensure device identity
    let device = ensure_device(&config, ws_url).await?;
    let device_id_value = device.device_id;
    info!("Device ID: {}", device_id_value);

    // Select character from filesystem
    let server_dir = config.server_dir(ws_url);
    let initial_character = select_character(&server_dir);

    // Start HTTP API FIRST (before character check) so web panel is accessible
    let device_id = Arc::new(RwLock::new(device_id_value));
    let (reconnect_tx, _reconnect_rx) =
        tokio::sync::broadcast::channel::<cyber_jianghu_agent::infra::api::ReconnectRequest>(64);

    // Early HTTP API startup based on mode
    let _early_api_state: Option<Arc<cyber_jianghu_agent::infra::api::HttpApiState>>;
    let early_actual_port: u16;

    match runtime_mode {
        RuntimeMode::Cognitive => {
            let (api_state, actual_port) = start_http_api_server(
                port,
                device_id.clone(),
                &config,
                ws_url,
                &device,
                server_dir.clone(),
                Some(reconnect_tx.clone()),
            )?;
            info!("HTTP API 已启动: http://localhost:{}", actual_port);
            info!("Web 面板: http://localhost:{}/", actual_port);
            info!("角色管理: http://localhost:{}/index.html", actual_port);
            _early_api_state = Some(api_state);
            early_actual_port = actual_port;
        }
        RuntimeMode::Claw => {
            let setup = start_claw_server(
                port,
                device_id.clone(),
                &config,
                ws_url,
                &device,
                server_dir.clone(),
            )?;
            _early_api_state = Some(Arc::new(setup.api_state.clone()));
            early_actual_port = setup.actual_port;
            info!(
                "Claw HTTP API 已启动: http://localhost:{}",
                early_actual_port
            );
        }
    }

    // Now check if we need to wait for character creation
    let character = match initial_character {
        Some(c) if c.agent_id.is_some() && c.status == CharacterStatus::Alive => c,
        _ => {
            info!("尚未创建角色，等待角色创建...");
            info!(
                "请通过 Web 面板创建角色: http://localhost:{}/index.html",
                early_actual_port
            );
            await_character_loop(&server_dir).await?;
            // After waiting, character MUST exist
            select_character(&server_dir).context("Character not found after waiting")?
        }
    };

    let data_dir = server_dir
        .join("characters")
        .join(character.agent_id.unwrap().to_string())
        .join("data");

    let device_id = Arc::new(RwLock::new(device_id_value));

    let persona_info = Some(cyber_jianghu_agent::soul::reflector::PersonaInfo {
        gender: character.gender.clone(),
        age: character.age,
        personality: character.personality.clone(),
        values: character.values.clone(),
    });

    // 根据模式创建决策回调和相关组件
    let maybe_callback_setup: Option<ClawCallbackSetup>;
    let cognitive_death_event_tx: Option<tokio::sync::broadcast::Sender<ServerMessage>>;
    let cognitive_api_state: Option<Arc<cyber_jianghu_agent::infra::api::HttpApiState>>;

    let mut agent = match runtime_mode {
        RuntimeMode::Cognitive => {
            info!("创建 Cognitive 模式组件...");
            let llm_client = create_llm_client(&config.llm)?;
            info!(
                "LLM 配置: provider={}, model={}",
                config.llm.provider,
                config.llm.model.as_deref().unwrap_or("default")
            );

            // 构建 LLM 客户端列表（主模型 + fallback）
            let mut llm_clients: Vec<Arc<dyn LlmClient>> = vec![Arc::new(llm_client)];
            for (i, fallback_model) in config.llm.fallback_models.iter().enumerate() {
                match create_llm_client_with_model(&config.llm, fallback_model) {
                    Ok(client) => {
                        info!("Fallback 模型 #{}: {}", i + 1, fallback_model);
                        llm_clients.push(Arc::new(client));
                    }
                    Err(e) => warn!(
                        "Fallback 模型 #{} ({}) 创建失败: {}",
                        i + 1,
                        fallback_model,
                        e
                    ),
                }
            }

            let llm_arc: Arc<dyn LlmClient> = if llm_clients.len() > 1 {
                Arc::new(FallbackLlmClient::new(llm_clients))
            } else {
                llm_clients.into_iter().next().unwrap()
            };
            let llm_container = Arc::new(RwLock::new(llm_arc.clone()));
            info!("LLM Client 容器已创建（支持热重载 + fallback）");

            let agent_name = character.name.as_str();
            let agent_id = device.device_id;

            let persona_description = character.generate_system_prompt();

            let cognitive_config = CognitiveEngineConfig {
                agent_name: agent_name.to_string(),
                persona: cyber_jianghu_agent::component::persona::DynamicPersona::new(
                    agent_id,
                    agent_name,
                    &persona_description,
                ),
                temperature: config.llm.temperature,
                max_tokens_per_stage: config.llm.max_tokens,
            };
            let cognitive_engine =
                Arc::new(CognitiveEngine::new(llm_arc.clone(), cognitive_config));
            let cognitive_engine_for_builder = cognitive_engine.clone();

            let cognitive_decision_with_feedback: DecisionWithFeedbackCallback =
                Arc::new(cognitive_decision_with_retry(
                    agent_id,
                    cognitive_engine.clone(),
                    CognitiveDecisionConfig::default().max_retries,
                ));

            // 带记忆上下文的决策回调（让记忆真正注入认知流程）
            let cognitive_engine_for_memory = cognitive_engine.clone();

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
            let decision_with_memory: cyber_jianghu_agent::runtime::DecisionWithMemoryCallback =
                Arc::new(move |ws: &WorldState, memory_context: &str| {
                    let engine = cognitive_engine_for_memory.clone();
                    let ws = ws.clone();
                    let memory_context = memory_context.to_string();
                    Box::pin(async move {
                        match engine.think_with_memory(&ws, &memory_context).await {
                            Ok(chain) => chain.final_intent,
                            Err(e) => {
                                error!("[cognitive] Decision with memory failed: {}", e);
                                Intent::new(
                                    ws.agent_id.unwrap_or_default(),
                                    ws.tick_id,
                                    "idle",
                                    None,
                                )
                                .with_thought(format!("认知失败: {}", e))
                            }
                        }
                    })
                });

            // Reuse early's api_state (prevents duplicate HTTP server startup)
            let early_api_state = _early_api_state
                .as_ref()
                .expect("early api_state must exist");

            // Reuse early's broadcast reconnect_tx by subscribing
            // This allows multiple consumers (Late Cognitive Agent) to receive reconnect events
            let reconnect_rx_for_builder = early_api_state
                .reconnect_tx
                .as_ref()
                .map(|tx| tx.subscribe())
                .unwrap();

            let api_state = early_api_state.clone();
            let browser_url = format!("http://localhost:{}/welcome.html", early_actual_port);
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

            // 初始化关系存储
            let agent_id_for_rel = character.agent_id.unwrap_or_else(Uuid::new_v4);
            let relationship_db_path = data_dir.join("relationships.db");
            let relationship_store =
                match cyber_jianghu_agent::component::social::RelationshipStore::open(
                    agent_id_for_rel,
                    &relationship_db_path,
                ) {
                    Ok(store) => {
                        info!("RelationshipStore 已初始化");
                        Some(store)
                    }
                    Err(e) => {
                        tracing::warn!("RelationshipStore 初始化失败: {}，继续无关系存储", e);
                        None
                    }
                };

            let mut builder = AgentBuilder::new(config_for_builder, decision)
                .device_config(device.clone())
                .data_dir(data_dir.clone())
                .with_decision_feedback(cognitive_decision_with_feedback)
                .with_decision_memory(decision_with_memory)
                .with_llm_container(llm_container)
                .with_llm_client(llm_arc.clone(), None)
                .with_http_api_state(api_state.clone())
                .with_reconnect_rx(reconnect_rx_for_builder);

            // 天魂 (IntentTranslator): Cognitive 模式专用，将人魂叙事翻译为格式化 Intent
            let intent_translator = Arc::new(IntentTranslator::new(llm_arc.clone()));
            builder = builder.with_intent_translator(intent_translator);
            info!("天魂 (IntentTranslator) 已创建");

            if let Some(store) = relationship_store {
                builder = builder.with_relationship_store(store);
            }

            builder = builder.cognitive_engine(cognitive_engine_for_builder.clone());

            builder = builder.character_config(character.clone());

            let agent = builder.build();

            maybe_callback_setup = None;
            cognitive_death_event_tx = Some(api_state.death_event_tx.clone());
            cognitive_api_state = Some(api_state.clone());
            agent
        }
        RuntimeMode::Claw => {
            info!("创建 Claw 模式组件...");
            let setup = start_claw_server(
                port,
                device_id.clone(),
                &config,
                ws_url,
                &device,
                server_dir.clone(),
            )?;
            cognitive_death_event_tx = None;
            cognitive_api_state = None;
            maybe_callback_setup = Some(ClawCallbackSetup {
                shared_state: setup.shared_state.clone(),
                api_state: setup.api_state.clone(),
                server_msg_tx: setup.server_msg_tx.clone(),
                device_id: device_id.clone(),
                persona_info: persona_info.clone(),
            });

            // 检查是否使用统一认知架构
            let use_unified_cognitive = config.claw.use_unified_cognitive;
            info!("Claw 模式 use_unified_cognitive={}", use_unified_cognitive);

            if use_unified_cognitive {
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

                // 创建 CognitiveEngine（与 Cognitive 模式共享架构）
                let agent_name = character.name.as_str();
                let agent_id = device.device_id;
                let persona_description = character.generate_system_prompt();

                let cognitive_config = CognitiveEngineConfig {
                    agent_name: agent_name.to_string(),
                    persona: cyber_jianghu_agent::component::persona::DynamicPersona::new(
                        agent_id,
                        agent_name,
                        &persona_description,
                    ),
                    temperature: config.llm.temperature,
                    max_tokens_per_stage: config.llm.max_tokens,
                };

                let llm_client: Arc<dyn LlmClient> = openclaw_bridge;
                let cognitive_engine =
                    Arc::new(CognitiveEngine::new(llm_client.clone(), cognitive_config));
                info!("CognitiveEngine 已创建（Claw 模式统一认知架构）");

                let cognitive_decision_with_feedback: DecisionWithFeedbackCallback =
                    Arc::new(cognitive_decision_with_retry(
                        agent_id,
                        cognitive_engine.clone(),
                        CognitiveDecisionConfig::default().max_retries,
                    ));

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

                // 使用 AgentBuilder 与 Cognitive 模式保持一致（COI 原则）
                let mut builder = AgentBuilder::new(config_for_builder.clone(), decision)
                    .device_config(device.clone())
                    .data_dir(data_dir.clone())
                    .with_decision_feedback(cognitive_decision_with_feedback)
                    .with_reconnect_rx(setup.reconnect_rx)
                    .with_llm_client(llm_client.clone(), None);

                // 天魂: Claw unified cognitive 模式也使用三魂架构
                let intent_translator = Arc::new(IntentTranslator::new(llm_client.clone()));
                builder = builder.with_intent_translator(intent_translator);

                builder = builder.character_config(character.clone());

                builder.build()
            } else {
                // === Legacy 路径（http_decision） ===
                // Agent 被动等待 OpenClaw 提交完整 Intent，不使用认知引擎
                info!("使用 Legacy 路径，等待 OpenClaw 提交 Intent");

                let agent_id = device.device_id;

                let decision: DecisionCallback = Arc::new(move |ws: &WorldState| {
                    let agent_id = agent_id;
                    let tick_id = ws.tick_id;
                    Box::pin(async move {
                        Intent::new(agent_id, tick_id, "idle", None)
                            .with_thought("等待 OpenClaw 提交 Intent".to_string())
                    })
                });

                let mut builder = AgentBuilder::new(config_for_builder.clone(), decision)
                    .device_config(device.clone())
                    .data_dir(data_dir.clone())
                    .with_reconnect_rx(setup.reconnect_rx);

                builder = builder.character_config(character.clone());

                builder.build()
            }
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

    // 外层循环：run() 返回 Ok(()) 表示需要重启（等待转生后重新连接）
    // Err 才是真正的致命错误
    // 支持 SIGTERM / Ctrl+C 优雅关闭
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);

    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        info!("收到 Ctrl+C 信号");
        let _ = shutdown_tx_clone.send(()).await;
    });

    #[cfg(unix)]
    {
        let shutdown_tx_clone = shutdown_tx.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            let mut sigterm =
                signal(SignalKind::terminate()).expect("Failed to install SIGTERM handler");
            sigterm.recv().await;
            info!("收到 SIGTERM 信号");
            let _ = shutdown_tx_clone.send(()).await;
        });
    }

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("正在优雅关闭 Agent...");
                agent.close().await.ok();
                info!("Agent 已关闭");
                break Ok(());
            }
            result = agent.run() => {
                if let Err(e) = result {
                    error!("Agent run() 错误: {}", e);
                }
                info!("Agent run() completed, restarting...");
            }
        }
    }
}

struct ServerSetup {
    reconnect_rx:
        tokio::sync::broadcast::Receiver<cyber_jianghu_agent::infra::api::ReconnectRequest>,
    server_msg_tx: tokio::sync::broadcast::Sender<DownstreamMessage>,
    shared_state: Arc<WsSharedState>,
    api_state: cyber_jianghu_agent::infra::api::HttpApiState,
    actual_port: u16,
}

struct ClawCallbackSetup {
    shared_state: Arc<WsSharedState>,
    api_state: cyber_jianghu_agent::infra::api::HttpApiState,
    server_msg_tx: tokio::sync::broadcast::Sender<DownstreamMessage>,
    device_id: Arc<RwLock<Uuid>>,
    persona_info: Option<cyber_jianghu_agent::soul::reflector::PersonaInfo>,
}

fn start_claw_server(
    port: u16,
    device_id: Arc<RwLock<Uuid>>,
    config: &Config,
    ws_url: &str,
    device: &DeviceConfig,
    server_dir: PathBuf,
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
    print_startup_banner(actual_port, ws_url, &config_path_str, "Claw");

    let (reconnect_tx, reconnect_rx) =
        tokio::sync::broadcast::channel::<cyber_jianghu_agent::infra::api::ReconnectRequest>(64);

    let mut ws_state = WsDecisionState::new();
    let shared_state = Arc::new(WsSharedState::from(&ws_state));
    ws_state.spawn_validation_task((*shared_state).clone());

    // Derive HTTP URL from WS URL
    let http_url = cyber_jianghu_agent::config::ws_to_http_url(ws_url);

    let character_dir = server_dir.join("characters");
    let (_http_decision_state, api_state) = create_http_state(
        device_id,
        http_url.to_string(),
        ws_url.to_string(),
        Some(device.clone()),
        server_dir,
        character_dir,
        Some(reconnect_tx),
        config_path(),
        Some(shared_state.clone()),
        config.runtime.mode,
        actual_port,
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
        actual_port,
    })
}

#[allow(clippy::too_many_arguments)]
fn start_http_api_server(
    port: u16,
    device_id: Arc<RwLock<Uuid>>,
    config: &Config,
    ws_url: &str,
    device: &DeviceConfig,
    server_dir: PathBuf,
    reconnect_tx: Option<
        tokio::sync::broadcast::Sender<cyber_jianghu_agent::infra::api::ReconnectRequest>,
    >,
) -> Result<(Arc<cyber_jianghu_agent::infra::api::HttpApiState>, u16)> {
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
    print_startup_banner(actual_port, ws_url, &config_path_str, "Cognitive");

    // Derive HTTP URL from WS URL
    let http_url = cyber_jianghu_agent::config::ws_to_http_url(ws_url);

    let character_dir = server_dir.join("characters");
    let (_http_decision_state, api_state) = cyber_jianghu_agent::runtime::create_http_state(
        device_id,
        http_url.to_string(),
        ws_url.to_string(),
        Some(device.clone()),
        server_dir,
        character_dir,
        reconnect_tx,
        config_path(),
        None,
        config.runtime.mode,
        actual_port,
    );

    let api_state_clone = api_state.clone();
    let is_auto_port = port == 0;
    let resolved_port = Arc::new(tokio::sync::Mutex::new(actual_port));
    let resolved_port_clone = resolved_port.clone();
    tokio::spawn(async move {
        let mut try_port = actual_port;

        loop {
            match cyber_jianghu_agent::runtime::run_http_server(try_port, api_state_clone.clone())
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
