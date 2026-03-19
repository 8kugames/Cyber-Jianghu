// ============================================================================
// Cyber-Jianghu Agent CLI
// ============================================================================
//
// 连接赛博江湖游戏世界的 Agent CLI
//
// 使用方式：
// 1. 首次运行：cyber-jianghu-agent setup --server ws://IP:PORT/ws --token YOUR_TOKEN --name 侠客名
// 2. 运行：cyber-jianghu-agent run --mode cognitive
// 3. 运行（HTTP 模式）：cyber-jianghu-agent run --mode http --port 23333
//
// 支持的决策模式：
// - cognitive: 使用内置多阶段认知引擎决策（直接调用 LLM API）
// - http: 启动 HTTP API 服务，供外部程序（如 OpenClaw）控制
// ============================================================================

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{Level, error, info};
use uuid::Uuid;

use cyber_jianghu_agent::config::{AgentConfig, AgentRole, MemoryConfig, PersonaConfig, ServerConfig};
use cyber_jianghu_agent::{
    Agent, Config, Intent, WorldState,
    ai::llm::{DirectLlmClient, DirectLlmClientConfig, LlmProvider},
    ai::persona::DynamicPersona,
    core::{CognitiveEngineConfig, MultiStageCognitiveEngine},
    runtime::decision::{create_http_state, http_decision, run_http_server},
};

// ============================================================================
// CLI 定义
// ============================================================================

#[derive(Parser)]
#[command(name = "cyber-jianghu-agent")]
#[command(about = "赛博江湖 Agent CLI - 连接游戏世界", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// 首次安装配置
    Setup {
        /// 服务端 WebSocket 地址
        #[arg(short, long)]
        server: String,
        /// Agent 名称
        #[arg(short, long)]
        name: String,
        /// 认证令牌
        #[arg(short, long)]
        token: String,
    },
    /// 更新服务器地址
    Config {
        /// 服务端 WebSocket 地址
        #[arg(short, long)]
        server: Option<String>,
        /// 认证令牌
        #[arg(short, long)]
        token: Option<String>,
    },
    /// 运行 Agent
    Run {
        /// 决策模式：cognitive（多阶段认知引擎）或 http（HTTP API 服务）
        #[arg(short, long, default_value = "cognitive")]
        mode: String,

        /// 监听端口（仅 mode=http 时有效）
        /// 0 = 在 23340~23349 范围内随机选择（推荐，避免与服务器端口 23333 冲突）
        /// 默认: 0
        #[arg(long, default_value = "0")]
        port: u16,

        /// === Cognitive 模式选项 ===

        /// LLM Provider: openclaw（使用宿主 OpenClaw）、openai_compatible（兼容接口，需指定 URL 和模型）、ollama（本地）(默认: openclaw)
        #[arg(long, default_value = "openclaw")]
        llm_provider: String,

        /// LLM API Key（仅 openai_compatible 需要）
        ///
        /// 优先使用环境变量：OPENAI_API_KEY
        #[arg(long)]
        api_key: Option<String>,

        /// LLM API Base URL（openai_compatible 必须指定，其他可选）
        #[arg(long)]
        base_url: Option<String>,

        /// LLM 模型名称（openai_compatible 必须指定，其他可选）
        #[arg(long)]
        model: Option<String>,
    },
    /// 显示当前配置
    Show,
}

// ============================================================================
// 配置管理
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

fn load_or_create_config() -> Result<Config> {
    let path = config_path();

    if path.exists() {
        info!("加载配置: {}", path.display());
        Config::from_file(&path).context("Failed to load config")
    } else {
        // 配置文件不存在时，尝试从环境变量加载
        info!("配置文件不存在，尝试从环境变量加载配置");
        Config::from_env().context("Failed to load config from environment variables")
    }
}

fn save_config(config: &Config) -> Result<()> {
    let path = config_path();

    // 创建目录
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // 生成 YAML
    let yaml = serde_yaml::to_string(config)?;
    std::fs::write(&path, yaml)?;
    info!("配置已保存到: {}", path.display());
    Ok(())
}

use futures_util::future::BoxFuture;

// ============================================================================
// 决策回调
// ============================================================================

// ============================================================================
// Cognitive 模式选项
// ============================================================================

/// 认知模式运行选项
#[derive(Debug, Clone, Default)]
struct RunOptions {
    /// LLM Provider
    pub llm_provider: String,
    /// LLM API Key
    pub api_key: Option<String>,
    /// LLM Base URL
    pub base_url: Option<String>,
    /// 模型名称
    pub model: Option<String>,
}

// ============================================================================
// HTTP Mode (Headless - for OpenClaw)
// ============================================================================
// HTTP API 实现已移至 runtime/decision/http.rs
// 使用 create_http_state() 和 run_http_server() 函数
// 包含基础端点 + AI 组件端点 (relationship, lifespan, memory, validate)
// ============================================================================
// 主入口
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志 - 默认输出到 stderr，以免干扰 stdout 模式下的通信
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Setup {
            server,
            name,
            token,
        }) => {
            info!("初始化配置...");

            let config = Config {
                agent: AgentConfig {
                    name,
                    system_prompt: "你是一位行走江湖的侠客。".to_string(),
                    persona: PersonaConfig::default(),
                    memory: MemoryConfig::default(),
                    role: AgentRole::default(),
                    review: None,
                    observer: None,
                },
                server: ServerConfig {
                    ws_url: server,
                    auth_token: token,
                },
                memory: MemoryConfig::default(),
                game_rules: None,
            };

            save_config(&config)?;
            info!("配置完成！运行 `cyber-jianghu-agent run` 启动 Agent");
        }

        Some(Commands::Config { server, token }) => {
            let mut config = load_or_create_config()?;

            if let Some(s) = server {
                config.server.ws_url = s;
            }
            if let Some(t) = token {
                config.server.auth_token = t;
            }

            save_config(&config)?;
            info!("配置已更新");
        }

        Some(Commands::Show) => {
            let config = load_or_create_config()?;
            println!("当前配置:");
            println!("  Agent名称: {}", config.agent.name);
            println!("  服务器地址: {}", config.server.ws_url);
            println!(
                "  Token: {}...",
                &config
                    .server
                    .auth_token
                    .chars()
                    .take(10)
                    .collect::<String>()
            );
        }

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

        None => {
            // 默认运行 cognitive 模式
            run_agent("cognitive", 0, RunOptions::default()).await?;
        }
    }

    Ok(())
}

async fn run_agent(mode: &str, port: u16, options: RunOptions) -> Result<()> {
    let config = load_or_create_config()?;
    info!("启动 Agent: {}", config.agent.name);
    info!("连接: {}", config.server.ws_url);

    // 创建共享的 agent_id（临时值，注册后会更新为服务器分配的真正 ID）
    let agent_id = Arc::new(RwLock::new(Uuid::new_v4()));

    // 如果是 http 模式，启动 HTTP Server（使用 runtime/decision/http.rs 的实现）
    let (http_decision_state, http_api_state) = if mode == "http" {
        // 如果 port 为 0，在 23340~23349 范围内随机选择端口
        let actual_port = if port == 0 {
            use rand::RngExt;
            let random_port = rand::rng().random_range(23340..=23349);
            info!("随机选择端口: {} (范围: 23340-23349)", random_port);
            random_port
        } else {
            port
        };

        info!("启动 HTTP 模式，端口: {}", actual_port);

        // 使用 create_http_state 创建 HTTP 决策状态
        let (decision_state, api_state) = create_http_state(agent_id.clone());

        // 启动 HTTP API 服务器
        let api_state_clone = api_state.clone();
        tokio::spawn(async move {
            if let Err(e) = run_http_server(actual_port, api_state_clone).await {
                error!("HTTP server error: {}", e);
            }
        });

        (Some(decision_state), Some(api_state))
    } else {
        (None, None)
    };

    // 选择决策模式
    let decision: Arc<dyn Fn(&WorldState) -> BoxFuture<'static, Intent> + Send + Sync> = match mode
    {
        "http" => {
            // HTTP 模式：使用 runtime/decision/http.rs 的 http_decision
            let state = http_decision_state.unwrap();
            Arc::new(http_decision(agent_id.clone(), state, 55))
        }
        "cognitive" => {
            // Cognitive 模式：使用多阶段认知引擎 + 直接 LLM API
            info!("启动 Cognitive 模式");
            info!("LLM Provider: {}", options.llm_provider);

            // 解析 LLM Provider
            let provider = LlmProvider::from_str(&options.llm_provider).context(format!(
                "Unknown LLM provider: {}. Valid options: openclaw, openai_compatible, ollama",
                options.llm_provider
            ))?;

            // 获取 API Key（如果 provider 需要）
            let api_key = if provider.requires_api_key() {
                let key = options
                    .api_key
                    .or_else(|| match provider {
                        LlmProvider::OpenAICompatible => std::env::var("OPENAI_API_KEY").ok(),
                        _ => None,
                    })
                    .context(format!(
                        "Missing API key for {}. Set --api-key or use environment variable ({})",
                        options.llm_provider,
                        match provider {
                            LlmProvider::OpenAICompatible => "OPENAI_API_KEY",
                            _ => "none",
                        }
                    ))?;
                Some(key)
            } else {
                None
            };

            // 构建 Direct LLM 客户端配置
            let mut client_config = DirectLlmClientConfig::new(provider, api_key);

            if let Some(ref base_url) = options.base_url {
                info!("使用自定义 Base URL: {}", base_url);
                client_config = client_config.with_base_url(base_url);
            }

            if let Some(ref model) = options.model {
                info!("使用模型: {}", model);
                client_config = client_config.with_model(model);
            }

            // 创建 Direct LLM 客户端（会自动验证配置）
            let llm_client = match DirectLlmClient::new(client_config) {
                Ok(client) => Arc::new(client),
                Err(e) => {
                    // 提供更友好的错误信息
                    if provider.requires_base_url() && options.base_url.is_none() {
                        anyhow::bail!(
                            "Provider 'openai_compatible' requires --base-url and --model to be specified.\n  Example: --llm-provider openai_compatible --base-url https://api.openai.com/v1 --model gpt-4"
                        );
                    }
                    if provider.requires_model() && options.model.is_none() {
                        anyhow::bail!(
                            "Provider 'openai_compatible' requires --model to be specified.\n  Example: --llm-provider openai_compatible --base-url https://api.openai.com/v1 --model gpt-4"
                        );
                    }
                    if provider == LlmProvider::OpenClaw {
                        let err_msg = e.to_string();
                        if err_msg.contains("Failed to read OpenClaw config") {
                            anyhow::bail!(
                                "Failed to load OpenClaw configuration from ~/.openclaw/openclaw.json.\n\
                                 Ensure OpenClaw is properly configured.\n\
                                 \n\
                                 Alternatively, specify --base-url to use a custom Gateway URL:\n\
                                 --llm-provider openclaw --base-url http://your-gateway:port"
                            );
                        }
                    }
                    return Err(e.context("Failed to create LLM client"));
                }
            };

            info!("LLM 客户端创建成功");
            info!("使用模型: {}", llm_client.model_name());

            // 创建认知引擎配置
            let agent_id = uuid::Uuid::new_v4();
            let dynamic_persona =
                DynamicPersona::new(agent_id, &config.agent.name, &config.agent.system_prompt);
            let engine_config = CognitiveEngineConfig {
                agent_name: config.agent.name.clone(),
                persona: dynamic_persona,
                temperature: 0.7,
                max_tokens_per_stage: 1024,
            };

            // 创建多阶段认知引擎
            let cognitive_engine = Arc::new(MultiStageCognitiveEngine::new(
                llm_client.clone(),
                engine_config,
            ));

            // 创建决策回调
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
            // 返回一个默认的空闲决策函数
            let mode_string = mode.to_string();
            Arc::new(move |world_state: &WorldState| {
                let tick_id = world_state.tick_id;
                let agent_id = world_state.agent_id.unwrap_or_default();
                let mode = mode_string.clone();
                Box::pin(async move {
                    error!(
                        "Unknown mode: {}. Supported modes: cognitive, http. Defaulting to idle.",
                        mode
                    );
                    Intent::idle(agent_id, tick_id).with_thought(format!("未知模式: {}", mode))
                })
            })
        }
    };

    let mut agent = Agent::new(config, decision);

    // 如果是 http 模式，设置注册回调来更新共享的 agent_id
    if let Some(_api_state) = http_api_state {
        let agent_id_clone = agent_id.clone();
        agent.set_registration_callback(std::sync::Arc::new(move |server_agent_id: uuid::Uuid| {
            // 使用 block_in_place 在同步上下文中读写异步锁
            let old_id = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(agent_id_clone.read())
            });
            info!("更新 HTTP API agent_id: {} -> {}", *old_id, server_agent_id);

            let mut guard = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(agent_id_clone.write())
            });
            *guard = server_agent_id;
        }));
    }

    agent.run().await?;
    Ok(())
}
