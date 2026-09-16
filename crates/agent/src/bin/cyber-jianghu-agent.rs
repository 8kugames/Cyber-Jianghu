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

    /// 检查并安装 GitHub Release 新版本（只安装不重启，重启 agent 后生效）
    Update {
        /// 仅检查是否有新版本，不下载安装
        #[arg(long)]
        check_only: bool,
    },

    /// 重置 Agent 身份（慎用，会清除所有数据）
    Reset,
}

// ============================================================================
// 配置路径
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

        Some(Commands::Update { check_only }) => {
            cyber_jianghu_agent::infra::updater::run_cli_update(check_only).await?;
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

/// Waits for a valid character to appear in the characters directory.
/// HTTP API must be started before calling this function.
pub(crate) async fn await_character_loop(
    server_dir: &Path,
    config: &cyber_jianghu_agent::config::Config,
    api_state: Option<&Arc<cyber_jianghu_agent::infra::api::HttpApiState>>,
) -> Result<()> {
    let characters_dir = server_dir.join("characters");

    std::fs::create_dir_all(&characters_dir).context("Failed to create characters directory")?;

    // 自动注册倒计时布防：等待期面板（setup/status）显示剩余秒数引导人工注册，
    // 超时后自动生成角色（runtime.auto_register_timeout_secs，0 = 禁用）。
    // 与运行期 wait_for_rebirth 共用 auto_register 模块，口径一致。
    let timeout_secs = config.runtime.auto_register_timeout_secs;
    if let Some(state) = api_state {
        cyber_jianghu_agent::infra::api::auto_register::arm_deadline_if_absent(state, timeout_secs)
            .await;
    }

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
            if let Some(state) = api_state {
                cyber_jianghu_agent::infra::api::auto_register::clear_deadline(state).await;
            }
            return Ok(());
        }

        // 超时兑底：读共享截止时刻（与面板倒计时同一来源），到点自动生成角色。
        // 注册 handler 成功后会落盘 character.yaml，下一轮循环经 select_character 退出。
        if let Some(state) = api_state {
            let due = state
                .auto_register_deadline
                .read()
                .await
                .is_some_and(|d| std::time::Instant::now() >= d);
            if due {
                match cyber_jianghu_agent::infra::api::auto_register::auto_register_via_loopback(
                    state,
                )
                .await
                {
                    Ok(id) => info!("[auto-register] 自动注册成功: agent_id={}", id),
                    Err(e) => {
                        warn!(
                            "[auto-register] 自动注册失败: {}，{}s 后重试",
                            e, timeout_secs
                        );
                        cyber_jianghu_agent::infra::api::auto_register::rearm_deadline(
                            state,
                            timeout_secs,
                        )
                        .await;
                    }
                }
            }
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

#[path = "bin_support/bootstrap.rs"]
mod bootstrap;
#[path = "bin_support/cli_commands.rs"]
mod cli_commands;
#[path = "bin_support/run_agent.rs"]
mod run_agent;
#[path = "bin_support/server_support.rs"]
mod server_support;

use bootstrap::{
    config_path, create_character_via_api, ensure_device, init_tracing, load_config,
    print_startup_banner, save_config, select_character,
};
use cli_commands::{create_character_cli, reset_agent, show_config, update_server_config};
pub(crate) use run_agent::ServerSetup;
use run_agent::run_agent;
use server_support::{create_llm_client, is_port_available, pick_auto_port};
pub(crate) use server_support::{start_claw_server, start_http_api_server};
