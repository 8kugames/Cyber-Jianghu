// ============================================================================
// Cyber-Jianghu Agent CLI
// ============================================================================
//
// 连接虚境：江湖游戏世界的 Agent CLI
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
// 5. Web 面板：http://localhost:<端口>/
// 6. HTTP API：http://localhost:<端口>/api/v1/*
// ============================================================================

#![allow(deprecated, unused_imports)]

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

use cyber_jianghu_agent::config::{
    CharacterConfig, CharacterStatus, Config, DeviceConfig, LlmConfig, RuntimeMode,
};
use cyber_jianghu_agent::{
    AgentBuilder,
    component::llm::LlmClient,
    infra::api::thinking_log,
    runtime::claw::{BridgeConfig, OpenClawBridge},
    runtime::claw::{DownstreamMessage, WsDecisionState, WsSharedState, run_ws_server},
    runtime::create_http_state,
    runtime::{
        CognitiveDecisionConfig, DecisionCallback, DecisionWithChainCallback,
        cognitive_decision_with_chain,
    },
    soul::actor::{CognitiveEngine, CognitiveEngineConfig},
};
use cyber_jianghu_protocol::{EraSettings, Intent, ServerMessage, WorldBuildingRules, WorldState};

// ============================================================================
// CLI 定义
// ============================================================================

#[derive(Parser)]
#[command(name = "cyber-jianghu-agent")]
#[command(about = "虚境：江湖 Agent - 连接游戏世界", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// 运行 Agent（默认命令）
    Run {
        /// 监听端口
        /// 0 = 在 23340~23999 范围内随机选择（推荐，避免与服务器端口 23333 冲突）
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

/// 返回配置目录（优先 CYBER_JIANGHU_CONFIG_DIR 环境变量，回退 $HOME/.cyber-jianghu/config）
fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CYBER_JIANGHU_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cyber-jianghu")
        .join("config")
}

fn config_path() -> PathBuf {
    config_dir().join("agent.yaml")
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

    // 保存 narrative_config（设备连接时下发，供前端属性分类使用）
    // hash skip-optimization：与 prompt_templates 相同逻辑，hash 未变则跳过磁盘写入
    {
        let nc = &body["narrative_config"];
        let nc_hash = body["narrative_config_hash"].as_str();
        if !nc.is_null() {
            let cdir = config_dir();
            let _ = std::fs::create_dir_all(&cdir);
            let hash_path = cdir.join("narrative_config.hash");

            let should_save = match nc_hash {
                Some(new_hash) => match std::fs::read_to_string(&hash_path) {
                    Ok(old_hash) => old_hash.trim() != new_hash,
                    Err(_) => true,
                },
                None => true,
            };

            if should_save {
                match serde_json::to_string_pretty(nc) {
                    Ok(json) => {
                        let nc_path = cdir.join("narrative_config.json");
                        if let Err(e) = std::fs::write(&nc_path, &json) {
                            warn!("保存 narrative_config 失败: {}", e);
                        } else {
                            if let Some(hash) = nc_hash {
                                let _ = std::fs::write(&hash_path, hash);
                            }
                            info!("设备连接时已保存 narrative_config");
                        }
                    }
                    Err(e) => warn!("序列化 narrative_config 失败: {}", e),
                }
            } else {
                debug!("narrative_config skip: hash unchanged");
            }
        }
    }

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
    let data_dir = std::env::var("CYBER_JIANGHU_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".cyber-jianghu")
        });

    let thinking_log_path = thinking_log::init_thinking_log(&data_dir)?;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
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
    let config = load_config()?.ok_or_else(|| {
        anyhow::anyhow!("配置文件不存在（character_generation 为必填项，无法使用默认配置）")
    })?;

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
        println!("  通过 Web 面板创建: http://localhost:{}/", display_port);
        println!("  或通过 CLI: cyber-jianghu-agent create-character --name 名字");
    }

    println!("\n运行时配置:");
    println!("  模式: {:?}", config.runtime.mode);
    println!("  端口: {}", config.runtime.port);

    Ok(())
}

fn update_server_config(ws_url: Option<String>, http_url: Option<String>) -> Result<()> {
    let mut config = load_config()?.ok_or_else(|| {
        anyhow::anyhow!("配置文件不存在（character_generation 为必填项，无法使用默认配置）")
    })?;

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
            warn!("请确保 Agent 已启动并监听端口 {}", port);
            warn!("或通过 Web 面板创建角色: http://localhost:{}/", port);
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

// ============================================================================
// LLM 客户端工厂（Claw vs Cognitive 的唯一架构差异）
// ============================================================================

/// 创建 LLM 客户端
/// - Cognitive: 内置 FallbackLlmClient
/// - Claw: OpenClawBridge (外部 OpenClaw 调度器)
///   其他一切 agent 能力（记忆、关系、三魂）都应统一，不因模式而异
fn create_llm_client(
    runtime_mode: RuntimeMode,
    config: &Config,
    shared_state: Option<Arc<WsSharedState>>,
) -> Result<Arc<dyn cyber_jianghu_agent::component::llm::LlmClient>> {
    match runtime_mode {
        RuntimeMode::Cognitive => Ok(cyber_jianghu_agent::component::llm::build_fallback_client(
            &config.llm,
            config.llm.enable_streaming,
            Some(config.earth_soul.clone()),
        )?),
        RuntimeMode::Claw => {
            let upstream_tx = shared_state
                .expect("Claw mode needs shared_state")
                .upstream_tx
                .clone();
            let bridge = OpenClawBridge::new(upstream_tx, BridgeConfig::default());
            Ok(Arc::new(bridge) as Arc<dyn cyber_jianghu_agent::component::llm::LlmClient>)
        }
    }
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
            info!(
                "Character found: {} ({})",
                c.name,
                c.agent_id.expect("character must have agent_id")
            );
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
    let mut config = load_config()?.ok_or_else(|| {
        anyhow::anyhow!("配置文件不存在且无法从环境变量构造（character_generation 为必填项）")
    })?;

    // Fail Fast: 校验 EarthSoul 配置
    config
        .earth_soul
        .validate()
        .context("earth_soul 配置校验失败")?;

    // Ensure servers_dir is set (#[serde(default)] means it's empty after from_file)
    // 优先级：CYBER_JIANGHU_DATA_DIR 环境变量 > ~/.cyber-jianghu/servers
    if config.servers_dir.as_os_str().is_empty() {
        config.servers_dir = if let Ok(data_dir) = std::env::var("CYBER_JIANGHU_DATA_DIR") {
            info!("使用 CYBER_JIANGHU_DATA_DIR: {}", data_dir);
            PathBuf::from(data_dir).join("servers")
        } else {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".cyber-jianghu")
                .join("servers")
        };
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
    info!("Device ID: {}", device.device_id);

    // Select character from filesystem
    let server_dir = config.server_dir(ws_url);
    let initial_character = select_character(&server_dir);

    // Determine the runtime agent_id:
    // - If an alive character with a valid agent_id exists, use it (so the web panel
    //   correctly marks is_current in list_characters_handler).
    // - Otherwise fall back to the device UUID (agent not registered yet).
    let runtime_agent_id = if let Some(ref character) = initial_character
        && let Some(agent_uuid) = character.agent_id
    {
        info!("使用已有角色 UUID 作为运行时 agent_id: {}", agent_uuid);
        agent_uuid
    } else {
        device.device_id
    };

    // Arc-wrapped so HTTP handlers can read the current agent_id at any time.
    // This IS the state.agent_id that list_characters_handler compares against.
    let runtime_agent_id = Arc::new(RwLock::new(runtime_agent_id));
    let (reconnect_tx, _reconnect_rx) =
        tokio::sync::broadcast::channel::<cyber_jianghu_agent::infra::api::ReconnectRequest>(64);

    // Early HTTP API startup based on mode
    let _early_api_state: Option<Arc<cyber_jianghu_agent::infra::api::HttpApiState>>;
    let _early_claw_setup: Option<LateClawSetup>;
    let early_actual_port: u16;

    match runtime_mode {
        RuntimeMode::Cognitive => {
            let (api_state, actual_port) = start_http_api_server(
                port,
                runtime_agent_id.clone(),
                &config,
                ws_url,
                &device,
                server_dir.clone(),
                Some(reconnect_tx.clone()),
            )
            .await?;
            info!("HTTP API 已启动: http://localhost:{}", actual_port);
            info!("Web 面板: http://localhost:{}/", actual_port);
            info!("角色管理: http://localhost:{}/index.html", actual_port);
            _early_api_state = Some(api_state);
            _early_claw_setup = None;
            early_actual_port = actual_port;
        }
        RuntimeMode::Claw => {
            let setup = start_claw_server(
                port,
                runtime_agent_id.clone(),
                &config,
                ws_url,
                &device,
                server_dir.clone(),
            )?;
            _early_api_state = Some(setup.api_state.clone());
            _early_claw_setup = Some(LateClawSetup {
                shared_state: setup.shared_state.clone(),
                api_state: setup.api_state.clone(),
                server_msg_tx: setup.server_msg_tx.clone(),
            });
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
        .join(
            character
                .agent_id
                .expect("character must have agent_id")
                .to_string(),
        )
        .join("data");

    let persona_info = Some(cyber_jianghu_agent::soul::reflector::PersonaInfo {
        name: Some(character.name.clone()),
        gender: character.gender.clone(),
        age: character.age,
        personality: character.personality.clone(),
        values: character.values.clone(),
    });

    // 根据模式创建决策回调和相关组件
    let maybe_callback_setup: Option<CallbackSetup>;
    let cognitive_death_event_tx: Option<tokio::sync::broadcast::Sender<ServerMessage>>;

    // ========================================================================
    // 阶段 1: 按模式创建 LLM 客户端 — 这是两模式唯一差异点
    // ========================================================================
    #[allow(clippy::type_complexity)]
    let (llm_client, llm_container, api_state): (
        Arc<dyn cyber_jianghu_agent::component::llm::LlmClient>,
        Arc<tokio::sync::RwLock<Arc<dyn cyber_jianghu_agent::component::llm::LlmClient>>>,
        Arc<cyber_jianghu_agent::infra::api::HttpApiState>,
    ) = match runtime_mode {
        RuntimeMode::Cognitive => {
            let llm = create_llm_client(runtime_mode, &config, None)?;
            let llm_arc: Arc<dyn cyber_jianghu_agent::component::llm::LlmClient> = llm.clone();
            let container = Arc::new(RwLock::new(llm_arc.clone()));

            let early = _early_api_state
                .as_ref()
                .expect("early api_state must exist");
            let state = early.clone();

            // 浏览器打开 Web 面板
            let browser_url = format!("http://localhost:{}/", early_actual_port);
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

            maybe_callback_setup = Some(CallbackSetup {
                shared_state: None,
                api_state: state.clone(),
                server_msg_tx: None,
                runtime_agent_id: runtime_agent_id.clone(),
                persona_info: persona_info.clone(),
            });
            cognitive_death_event_tx = Some(state.death_event_tx.clone());

            (llm_arc, container, state)
        }
        RuntimeMode::Claw => {
            let setup = _early_claw_setup.expect("early claw setup must exist");
            let llm_response_rx = setup
                .shared_state
                .llm_response_rx
                .lock()
                .expect("lock poisoned")
                .take();

            let openclaw_bridge = Arc::new(OpenClawBridge::new(
                setup.shared_state.upstream_tx.clone(),
                BridgeConfig::default(),
            ));
            let llm: Arc<dyn cyber_jianghu_agent::component::llm::LlmClient> =
                openclaw_bridge.clone();
            let container = Arc::new(RwLock::new(llm.clone()));

            // LLM 响应转发任务（Claw 独有）
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

            maybe_callback_setup = Some(CallbackSetup {
                shared_state: Some(setup.shared_state.clone()),
                api_state: setup.api_state.clone(),
                server_msg_tx: Some(setup.server_msg_tx.clone()),
                runtime_agent_id: runtime_agent_id.clone(),
                persona_info: persona_info.clone(),
            });
            cognitive_death_event_tx = None;

            (llm, container, setup.api_state.clone())
        }
    };

    // ========================================================================
    // 阶段 2: 统一初始化 — 两模式共享的大脑
    // ========================================================================
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
    let mut engine = CognitiveEngine::new(llm_client.clone(), cognitive_config);

    // Outcome Memory（Hermes 模式）
    let outcome_db_path = data_dir.join("outcome_memory.db");
    match cyber_jianghu_agent::component::memory::OutcomeMemory::new(&outcome_db_path, 10) {
        Ok(mem) => {
            info!(
                "Outcome memory initialized at {}",
                outcome_db_path.display()
            );
            engine.set_outcome_memory(mem);
        }
        Err(e) => {
            warn!(
                "Failed to initialize outcome memory: {}. Running without it.",
                e
            );
        }
    }

    // Conversation History（长窗口对话）
    let conv_db_path = data_dir.join("conversation_history.db");
    match cyber_jianghu_agent::component::llm::conversation::ConversationHistory::new(
        &conv_db_path,
        &persona_description,
        config.llm.context_window_tokens as usize,
        config.llm.keep_recent_turns as usize,
        config.llm.summary_trigger_ratio,
    ) {
        Ok(history) => {
            info!(
                "Conversation history initialized at {} (max_tokens={}, keep_recent={})",
                conv_db_path.display(),
                config.llm.context_window_tokens,
                config.llm.keep_recent_turns,
            );
            engine.set_conversation_history(history);
        }
        Err(e) => {
            warn!(
                "Failed to initialize conversation history: {}. Running without it.",
                e
            );
        }
    }

    // 设置 NarrativeSummaryWindow 窗口大小
    engine.set_narrative_window_size(config.llm.narrative_window_size);

    // 设置流式 LLM
    engine.set_enable_streaming(config.llm.enable_streaming);

    let cognitive_engine = Arc::new(engine);

    // 决策回调
    let decision_with_chain: DecisionWithChainCallback = Arc::new(cognitive_decision_with_chain(
        cognitive_engine.clone(),
        CognitiveDecisionConfig::default().max_retries,
    ));

    let cognitive_engine_for_memory = cognitive_engine.clone();
    let cognitive_engine_for_decision = cognitive_engine.clone();
    let decision: DecisionCallback = Arc::new(move |tick_id: i64, agent_id: Uuid| {
        let engine = cognitive_engine_for_decision.clone();
        Box::pin(async move {
            match engine.think(tick_id, agent_id).await {
                Ok(chain) => chain.final_intent,
                Err(e) => {
                    error!("[cognitive] Decision failed: {}", e);
                    Intent::new(agent_id, tick_id, "休息", None)
                        .with_thought(format!("认知失败: {}", e))
                }
            }
        })
    });

    let decision_with_memory: cyber_jianghu_agent::runtime::DecisionWithMemoryCallback =
        Arc::new(move |tick_id: i64, agent_id: Uuid, memory_context: &str| {
            let engine = cognitive_engine_for_memory.clone();
            let memory_context = memory_context.to_string();
            Box::pin(async move {
                match engine
                    .think_with_memory(tick_id, agent_id, &memory_context)
                    .await
                {
                    Ok(chain) => chain.final_intent,
                    Err(e) => {
                        error!("[cognitive] Decision with memory failed: {}", e);
                        Intent::new(agent_id, tick_id, "休息", None)
                            .with_thought(format!("认知失败: {}", e))
                    }
                }
            })
        });

    // RelationshipStore（per-character DB，与 HTTP API 路径对齐）
    let agent_id_for_rel = character.agent_id.unwrap_or_else(Uuid::new_v4);
    let relationship_db_path = data_dir.join(format!("relationships_{}.db", agent_id_for_rel));
    let relationship_store = match cyber_jianghu_agent::component::social::RelationshipStore::open(
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

    // AgentBuilder
    let reconnect_rx = api_state
        .reconnect_tx
        .as_ref()
        .map(|tx| tx.subscribe())
        .expect("reconnect_tx must be initialized");

    let mut builder = AgentBuilder::new(config_for_builder, decision)
        .device_config(device.clone())
        .data_dir(data_dir.clone())
        .with_decision_chain(decision_with_chain)
        .with_decision_memory(decision_with_memory)
        .with_llm_container(llm_container.clone())
        .with_llm_client(
            llm_client.clone(),
            Some(WorldBuildingRules {
                version: String::new(),
                era: EraSettings {
                    name: String::new(),
                    tech_level: String::new(),
                    social_structure: String::new(),
                },
                allowed_concepts: Vec::new(),
                forbidden_concepts: Vec::new(),
                narrative_rules: String::new(),
                last_updated: String::new(),
                rules_json: None,
            }),
        )
        .with_http_api_state(api_state.clone())
        .with_reconnect_rx(reconnect_rx)
        .cognitive_engine(cognitive_engine.clone());

    // ChaosGenerator
    builder = builder.with_chaos_generator(cyber_jianghu_agent::soul::actor::ChaosGenerator::new(
        cyber_jianghu_agent::soul::actor::ChaosConfig::default(),
    ));

    if let Some(store) = relationship_store {
        builder = builder.with_relationship_store(store);
    }

    builder = builder.character_config(character.clone());

    // ImmediateHandler（即时事件处理：SQLite 持久化 + Session Triage LLM）
    {
        builder = builder.with_immediate_handler();
        info!("即时事件处理器已创建");
    }

    // DeltaEngine + AttentionController（Token 优化模式）
    let token_opt_enabled = config.token_optimization.enabled;
    let world_state_store =
        std::sync::Arc::new(cyber_jianghu_agent::component::state_store::WorldStateStore::new());

    if token_opt_enabled {
        let delta_config = cyber_jianghu_agent::component::delta_engine::DeltaConfig {
            change_percentage_threshold: config
                .token_optimization
                .delta
                .change_percentage_threshold,
            survival_critical_urgency_threshold: config
                .token_optimization
                .delta
                .survival_critical_urgency_threshold,
        };
        let attention_config = config.token_optimization.attention.clone();
        builder = builder
            .with_world_state_store(world_state_store.clone())
            .with_delta_engine(
                cyber_jianghu_agent::component::delta_engine::DeltaEngine::new(delta_config),
            )
            .with_attention_controller(
                cyber_jianghu_agent::component::attention::AttentionController::new(
                    attention_config,
                ),
            );
        info!("DeltaEngine + AttentionController 已初始化（Token 优化模式）");
    }

    let mut agent = builder.build();

    // 注入 world_state_store 到 HttpApiState（供 Claw 模式 Delta Engine 使用）
    if token_opt_enabled {
        *api_state
            .world_state_store
            .write()
            .expect("rwlock poisoned") = Some(world_state_store.clone());
        info!("world_state_store 已注入 HttpApiState");
    }

    // 注入 relationship_store 到 HttpApiState
    if let Some(store) = agent.relationship_store() {
        *api_state
            .relationship_store
            .write()
            .expect("rwlock poisoned") = Some(Arc::new(store.clone()));
        info!("relationship_store 已注入 HttpApiState");
    }

    // 注入 LLM container 到 HttpApiState（支持热重载重建）
    {
        *api_state.llm_container.write().await = Some(llm_container.clone());
        info!("LLM container 已注入 HttpApiState（支持热重载）");
    }

    // 注入 MemoryManager 到 HttpApiState（与 Agent 共享同一实例）
    if let Some(mm) = agent.memory_manager() {
        // mm is Arc<tokio::sync::RwLock<MemoryManager>>
        // Clone the Arc to share with HttpApiState
        let mm = Arc::clone(mm);
        *api_state.memory_manager.write().await = Some(mm);
        info!("MemoryManager 已注入 HttpApiState（与 Agent 共享）");
    } else {
        info!("Agent 未创建 MemoryManager");
    }

    // 死亡事件回调：Cognitive 模式通过 lifecycle 处理死亡标记
    if let Some(death_tx) = cognitive_death_event_tx {
        let death_tx_clone = death_tx.clone();
        let api_state_clone = api_state.clone();
        agent
            .set_server_msg_callback(std::sync::Arc::new(move |msg: ServerMessage| {
                if let ServerMessage::AgentDied {
                    rebirth_delay_ticks,
                    ..
                } = &msg
                {
                    api_state_clone
                        .is_dead
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    api_state_clone
                        .rebirth_delay_ticks
                        .store(*rebirth_delay_ticks, std::sync::atomic::Ordering::Relaxed);
                    let _ = death_tx_clone.send(msg);
                }
            }))
            .await;
    }

    // 注册回调 + Claw 模式 downstream 转发
    if let Some(setup) = maybe_callback_setup {
        let shared_state_clone = setup.shared_state.clone();
        let api_state_clone = setup.api_state.clone();
        let runtime_agent_id_clone = setup.runtime_agent_id.clone();
        let persona_clone = setup.persona_info.clone();

        agent.set_registration_callback(std::sync::Arc::new(move |server_agent_id: Uuid| {
            // block_in_place 允许在 multi-threaded runtime 的同步闭包中执行 async 操作
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    let old_id = *runtime_agent_id_clone.read().await;
                    info!("更新 runtime agent_id: {} -> {}", old_id, server_agent_id);
                    *runtime_agent_id_clone.write().await = server_agent_id;

                    // WsSharedState 注入 — 仅 Claw 模式有值
                    if let Some(ref shared_state) = shared_state_clone {
                        if let Some(ref validator) = api_state_clone.intent_validator {
                            let mut validator_guard = shared_state.intent_validator.write().await;
                            *validator_guard = Some(validator.clone());
                            info!("Validator injected into WsSharedState");
                        }
                        {
                            let game_rules = api_state_clone.game_rules.read().await.clone();
                            let mut guard = shared_state.game_rules.write().await;
                            *guard = game_rules;
                        }

                        if let Some(ref persona) = persona_clone {
                            let mut persona_guard = shared_state.persona.write().await;
                            *persona_guard = Some(persona.clone());
                            info!("Persona injected into WsSharedState");
                        }
                    }
                });
            });
        }));

        // server_msg_callback: Claw 模式做 downstream 转发
        if let Some(ref server_msg_tx) = setup.server_msg_tx {
            let tx_clone = server_msg_tx.clone();
            agent
                .set_server_msg_callback(std::sync::Arc::new(move |msg: ServerMessage| {
                    let current_tick = 0;
                    if let Some(downstream) =
                        DownstreamMessage::from_server_message(msg, current_tick)
                    {
                        let _ = tx_clone.send(downstream);
                    }
                }))
                .await;
        }
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
    server_msg_tx: tokio::sync::broadcast::Sender<DownstreamMessage>,
    shared_state: Arc<WsSharedState>,
    api_state: Arc<cyber_jianghu_agent::infra::api::HttpApiState>,
    actual_port: u16,
}

#[derive(Clone)]
struct LateClawSetup {
    shared_state: Arc<WsSharedState>,
    api_state: Arc<cyber_jianghu_agent::infra::api::HttpApiState>,
    server_msg_tx: tokio::sync::broadcast::Sender<DownstreamMessage>,
}

/// 统一的注册回调配置（Cognitive + Claw 模式共享）
/// Claw 模式独有字段为 Option，Cognitive 传入 None
struct CallbackSetup {
    /// WsSharedState — 仅 Claw 模式有值
    shared_state: Option<Arc<WsSharedState>>,
    api_state: Arc<cyber_jianghu_agent::infra::api::HttpApiState>,
    /// Downstream message tx — 仅 Claw 模式有值
    server_msg_tx: Option<tokio::sync::broadcast::Sender<DownstreamMessage>>,
    /// Runtime agent_id Arc — MUST be kept in sync with HttpApiState.agent_id.
    runtime_agent_id: Arc<RwLock<Uuid>>,
    persona_info: Option<cyber_jianghu_agent::soul::reflector::PersonaInfo>,
}

/// 检查端口是否可用（未被占用）
async fn is_port_available(port: u16) -> bool {
    tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .is_ok()
}

/// 自动选择可用端口，优先 23340
async fn pick_auto_port() -> u16 {
    const PREFERRED_PORT: u16 = 23340;
    const PORT_RANGE_START: u16 = 23340;
    const PORT_RANGE_END: u16 = 23999;

    // 优先尝试 23340
    if is_port_available(PREFERRED_PORT).await {
        info!("使用首选端口: {}", PREFERRED_PORT);
        return PREFERRED_PORT;
    }

    // 23340 被占用，随机选择其他端口
    use rand::RngExt;
    let mut rng = rand::rng();
    let available_ports: Vec<u16> = (PORT_RANGE_START..=PORT_RANGE_END)
        .filter(|&p| p != PREFERRED_PORT)
        .collect();

    // 随机打乱可用端口
    let random_idx = rng.random_range(0..available_ports.len());
    let selected_port = available_ports[random_idx];
    info!(
        "首选端口 {} 被占用，选择端口: {} (范围: {}-{}, 已排除 {})",
        PREFERRED_PORT, selected_port, PORT_RANGE_START, PORT_RANGE_END, PREFERRED_PORT
    );
    selected_port
}

fn start_claw_server(
    port: u16,
    runtime_agent_id: Arc<RwLock<Uuid>>,
    config: &Config,
    ws_url: &str,
    device: &DeviceConfig,
    server_dir: PathBuf,
) -> Result<ServerSetup> {
    let actual_port = if port == 0 {
        // 使用同步阻塞方式等待端口选择（避免 async trait 复杂化）
        tokio::runtime::Handle::current().block_on(pick_auto_port())
    } else {
        port
    };

    info!(
        "启动 Claw 模式（WebSocket + HTTP API），端口: {}",
        actual_port
    );

    let config_path_str = config_path().display().to_string();
    print_startup_banner(actual_port, ws_url, &config_path_str, "Claw");

    let (reconnect_tx, _) =
        tokio::sync::broadcast::channel::<cyber_jianghu_agent::infra::api::ReconnectRequest>(64);

    let ws_state = WsDecisionState::new();
    let shared_state = Arc::new(WsSharedState::from(&ws_state));
    // 统一认知模式下外部 Intent 已被 server.rs 拦截，无需启动验证任务
    // CAS 去重逻辑保留在 WsDecisionState 中作为通用安全机制
    // ws_state.spawn_validation_task((*shared_state).clone());

    // Derive HTTP URL from WS URL
    let http_url = cyber_jianghu_agent::config::ws_to_http_url(ws_url);

    let character_dir = server_dir.join("characters");
    let (_http_decision_state, api_state) = create_http_state(
        runtime_agent_id,
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
        server_msg_tx,
        shared_state: shared_state_for_callback,
        api_state: Arc::new(api_state_for_callback),
        actual_port,
    })
}

#[allow(clippy::too_many_arguments)]
async fn start_http_api_server(
    port: u16,
    runtime_agent_id: Arc<RwLock<Uuid>>,
    config: &Config,
    ws_url: &str,
    device: &DeviceConfig,
    server_dir: PathBuf,
    reconnect_tx: Option<
        tokio::sync::broadcast::Sender<cyber_jianghu_agent::infra::api::ReconnectRequest>,
    >,
) -> Result<(Arc<cyber_jianghu_agent::infra::api::HttpApiState>, u16)> {
    let port_range_start = 23340u16;
    let port_range_end = 23999u16;

    let actual_port = if port == 0 {
        pick_auto_port().await
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
        runtime_agent_id,
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
