// ============================================================================
// Cyber-Jianghu Agent CLI
// ============================================================================
//
// 连接赛博江湖游戏世界的 Agent CLI
//
// 使用方式：
// 1. 首次运行：自动生成 device_id 并向服务器注册
// 2. 后续运行：自动使用已保存的身份连接服务器
// 3. 通过 Web 面板创建角色：http://localhost:23340/panel
// 4. 通过 HTTP API 创建角色：POST /api/v1/character/register
//
// 支持的决策模式：
// - cognitive: 使用内置多阶段认知引擎决策（直接调用 LLM API）
// - http: 启动 HTTP API 服务，供外部程序（如 OpenClaw）控制
// ============================================================================

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{Level, error, info, warn};
use uuid::Uuid;

use cyber_jianghu_agent::config::{
    CharacterConfig, Config, IdentityConfig,
};
use cyber_jianghu_agent::{
    Agent, Intent, WorldState,
    ai::llm::{DirectLlmClient, DirectLlmClientConfig, LlmProvider},
    ai::persona::DynamicPersona,
    core::{CognitiveEngineConfig, MultiStageCognitiveEngine},
    runtime::decision::{create_http_state, http_decision},
    runtime::decision::ws::{
        run_ws_server, WsDecisionState, WsSharedState,
    },
};

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
        /// 决策模式：
        /// - claw: 为 OpenClaw 等外部助手提供 WebSocket + HTTP API（默认）
        /// - cognitive: 内置 LLM 决策（无外部接口）
        #[arg(short, long, default_value = "claw")]
        mode: String,

        /// 监听端口（claw 模式）
        /// 0 = 在 23340~23349 范围内随机选择（推荐，避免与服务器端口 23333 冲突）
        #[arg(long, default_value = "0")]
        port: u16,

        /// === Cognitive 模式选项 ===

        /// LLM Provider: openclaw、openai_compatible、ollama (默认: openclaw)
        #[arg(long, default_value = "openclaw")]
        llm_provider: String,

        /// LLM API Key（仅 openai_compatible 需要）
        #[arg(long)]
        api_key: Option<String>,

        /// LLM API Base URL（openai_compatible 必须指定）
        #[arg(long)]
        base_url: Option<String>,

        /// LLM 模型名称（openai_compatible 必须指定）
        #[arg(long)]
        model: Option<String>,
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
async fn create_character_via_api(
    agent_port: u16,
    character: CharacterConfig,
) -> Result<Uuid> {
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
    Uuid::parse_str(&result.agent_id)
        .context("Failed to parse agent_id as UUID")
}

// ============================================================================
// 确保 Agent 身份存在
// ============================================================================

async fn ensure_identity(config: &mut Config) -> Result<()> {
    if config.identity.is_some() {
        info!("使用已有 Agent 身份");
        return Ok(());
    }

    info!("首次启动，生成设备身份...");

    // 1. 生成 device_id
    let device_id = Uuid::new_v4();
    info!("生成设备 ID: {}", device_id);

    // 2. 向服务器注册
    let auth_token = register_agent_identity(&config.server.http_url, device_id).await?;

    // 3. 保存身份
    config.identity = Some(IdentityConfig {
        device_id,
        auth_token,
    });

    // 4. 持久化
    save_config(config)?;
    info!("Agent 身份已创建并保存");

    Ok(())
}

// ============================================================================
// 认知模式选项
// ============================================================================

#[derive(Debug, Clone, Default)]
struct RunOptions {
    pub llm_provider: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
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
        Some(Commands::Run {
            mode,
            port,
            llm_provider,
            api_key,
            base_url,
            model,
        }) => {
            run_agent(
                &mode,
                port,
                RunOptions {
                    llm_provider,
                    api_key,
                    base_url,
                    model,
                },
            )
            .await?;
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
            // 默认运行 HTTP 模式
            run_agent("http", 0, RunOptions::default()).await?;
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
        println!("Auth Token: {}...", &identity.auth_token.chars().take(16).collect::<String>());
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
            warn!("请确保 Agent 已在 HTTP 模式下运行");
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

// ============================================================================
// 运行 Agent
// ============================================================================

async fn run_agent(mode: &str, port: u16, options: RunOptions) -> Result<()> {
    // 1. 加载或创建配置
    let mut config = load_config()?.unwrap_or_else(|| {
        info!("配置文件不存在，从环境变量加载");
        Config::from_env().unwrap_or_default()
    });

    // 2. 确保 Agent 身份存在
    ensure_identity(&mut config).await?;

    let identity = config.identity.as_ref().expect("Identity should exist after ensure_identity");
    info!("Device ID: {}", identity.device_id);

    // 3. 检查是否已创建角色
    if !config.has_character() {
        warn!("尚未创建角色");
        warn!("请通过以下方式创建角色:");
        warn!("  1. Web 面板: http://localhost:23340/panel");
        warn!("  2. CLI: cyber-jianghu-agent create-character --name 名字");
        warn!("  3. HTTP API: POST /api/v1/character/register");
    }

    // 4. 创建共享的 device_id
    let device_id = Arc::new(RwLock::new(identity.device_id));

    // 5. 如果是 claw 模式，启动混合服务（WebSocket + HTTP API）
    let is_claw_mode = mode == "claw" || mode == "http";
    let claw_decision_state = if is_claw_mode {
        let actual_port = if port == 0 {
            use rand::RngExt;
            let random_port = rand::rng().random_range(23340..=23349);
            info!("随机选择端口: {} (范围: 23340-23349)", random_port);
            random_port
        } else {
            port
        };

        info!("启动 Claw 模式（WebSocket + HTTP API），端口: {}", actual_port);

        // 创建 HTTP API 状态（用于数据访问 API）
        let (http_decision_state, api_state) = create_http_state(
            device_id.clone(),
            config.server.http_url.clone(),
            Some(identity.clone()),
        );

        // 创建 WebSocket 决策状态（用于实时决策）
        let ws_state = WsDecisionState::new();
        let shared_state = WsSharedState::from(&ws_state);

        // 启动混合服务（WebSocket + HTTP API）
        let api_state_clone = api_state.clone();
        tokio::spawn(async move {
            if let Err(e) = run_ws_server(actual_port, shared_state, api_state_clone).await {
                error!("Claw server error: {}", e);
            }
        });

        // 返回 HTTP decision state（用于 http_decision）
        Some(http_decision_state)
    } else {
        None
    };

    // 6. 选择决策模式
    let decision: Arc<dyn Fn(&WorldState) -> futures_util::future::BoxFuture<'static, Intent> + Send + Sync> = match mode {
        "claw" | "http" => {
            let state = claw_decision_state.expect("claw_decision_state should exist in claw mode");
            Arc::new(http_decision(device_id.clone(), state, 55))
        }
        "cognitive" => {
            info!("启动 Cognitive 模式");
            info!("LLM Provider: {}", options.llm_provider);

            let provider = LlmProvider::from_str(&options.llm_provider).context(format!(
                "Unknown LLM provider: {}. Valid options: openclaw, openai_compatible, ollama",
                options.llm_provider
            ))?;

            let api_key = if provider.requires_api_key() {
                let key = options
                    .api_key
                    .or_else(|| std::env::var("OPENAI_API_KEY").ok())
                    .context(format!(
                        "Missing API key for {}. Set --api-key or OPENAI_API_KEY",
                        options.llm_provider
                    ))?;
                Some(key)
            } else {
                None
            };

            let mut client_config = DirectLlmClientConfig::new(provider, api_key);

            if let Some(ref base_url) = options.base_url {
                client_config = client_config.with_base_url(base_url);
            }

            if let Some(ref model) = options.model {
                client_config = client_config.with_model(model);
            }

            let llm_client = Arc::new(DirectLlmClient::new(client_config)?);
            info!("LLM 客户端创建成功，模型: {}", llm_client.model_name());

            // 获取角色信息（如果已创建）
            let (agent_name, system_prompt) = if let Some(ref character) = config.agent {
                (character.name.clone(), character.generate_system_prompt())
            } else {
                ("未命名Agent".to_string(), "你是一个普通的江湖人物".to_string())
            };

            let dynamic_persona = DynamicPersona::new(identity.device_id, &agent_name, &system_prompt);
            let engine_config = CognitiveEngineConfig {
                agent_name,
                persona: dynamic_persona,
                temperature: 0.7,
                max_tokens_per_stage: 1024,
            };

            let cognitive_engine = Arc::new(MultiStageCognitiveEngine::new(llm_client, engine_config));

            Arc::new(move |world_state: &WorldState| {
                let engine = cognitive_engine.clone();
                let agent_id = world_state.agent_id.unwrap_or_default();
                let tick_id = world_state.tick_id;
                let world_state_clone = world_state.clone();

                Box::pin(async move {
                    match engine.think(&world_state_clone).await {
                        Ok(chain) => chain.final_intent,
                        Err(e) => {
                            error!("认知流程失败: {}", e);
                            Intent::idle(agent_id, tick_id).with_thought(format!("认知失败: {}", e))
                        }
                    }
                })
            })
        }
        _ => {
            let mode_string = mode.to_string();
            Arc::new(move |world_state: &WorldState| {
                let tick_id = world_state.tick_id;
                let agent_id = world_state.agent_id.unwrap_or_default();
                let mode = mode_string.clone();
                Box::pin(async move {
                    error!("Unknown mode: {}. Supported: cognitive, http", mode);
                    Intent::idle(agent_id, tick_id).with_thought(format!("未知模式: {}", mode))
                })
            })
        }
    };

    // 7. 创建并运行 Agent
    let mut agent = Agent::new(config, decision);

    // 设置注册回调（仅 claw 模式需要）
    if is_claw_mode {
        let device_id_clone = device_id.clone();
        agent.set_registration_callback(std::sync::Arc::new(move |server_agent_id: Uuid| {
            let old_id = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(device_id_clone.read())
            });
            info!("更新 Claw API device_id: {} -> {}", *old_id, server_agent_id);

            let mut guard = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(device_id_clone.write())
            });
            *guard = server_agent_id;
        }));
    }

    agent.run().await?;
    Ok(())
}
