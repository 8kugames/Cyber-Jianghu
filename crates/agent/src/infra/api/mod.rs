// ============================================================================
// HTTP Decision - HTTP API 服务器（辅助功能）
// ============================================================================
//
// HTTP API 用于数据查询、Web 面板、调试等辅助功能。
// OpenClaw（外置大脑）必须通过 WebSocket 连接 Agent，确保 Tick 实时同步。
//
// 可用端点：
// - GET  /api/v1               - API 列表和使用规范（发现端点）
// - GET  /api/v1/health        - 健康检查
// - GET  /api/v1/state         - 获取当前 WorldState
// - GET  /api/v1/context       - 获取格式化的上下文（Markdown）
// - GET  /api/v1/attributes    - 梦中一瞥：获取属性数值（禁止存储到记忆）
// - POST /api/v1/intent        - 提交 Intent (已禁用，强制使用 WebSocket)
// - GET  /api/v1/relationship/list  - 获取所有关系
// - GET  /api/v1/relationship/{id}   - 获取特定关系
// - POST /api/v1/relationship       - 更新关系
// - GET  /api/v1/lifespan      - 获取寿命状态
// - GET  /api/v1/memory/recent - 获取近期记忆
// - POST /api/v1/memory/search - 搜索记忆（语义搜索待实现）
// - POST /api/v1/memory        - 存储记忆
// - POST /api/v1/validate      - 验证 Intent
// - GET  /api/v1/review/pending    - 获取待审查意图列表（Player Agent 提供）
// - POST /api/v1/review/{intent_id} - 提交审查结果（Observer Agent 调用）
// - GET  /api/v1/review/{intent_id}/status - 获取审查状态
// - GET/POST /api/v1/config/llm-disabled  - LLM 停止状态
// - GET/POST /api/v1/config/auto-rebirth  - 自动重生开关
//
// 架构设计：
// - 数据驱动 COI 原则：AI 组件都是可选注入，按需初始化
// - HTTP API 是辅助功能，WebSocket 是 OpenClaw 与 Agent 的主通道
// - 并发安全：所有可变状态都使用 tokio 的读写锁保护

pub mod auth;
pub mod auto_register;
pub mod cognitive_context;
mod context;
mod dto;
pub(crate) mod handlers;
pub mod service;
pub mod soul_cycle_recorder;
mod soul_cycle_types;
pub mod thinking_log;
pub mod trace;
#[cfg(test)]
mod warn_contract_tests;

use axum::{
    Router,
    routing::{get, post},
};
use cyber_jianghu_protocol::{Intent, ServerMessage, WorldState};
use futures_util::future::BoxFuture;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};
use tracing::{error, info};
use uuid::Uuid;

use anyhow::Context;

/// 重连请求（通过 channel 发送给主循环）
#[derive(Debug, Clone)]
pub struct ReconnectRequest {
    pub ws_url: String,
    pub agent_id: Option<Uuid>,
}

// 导入 handlers 中的 DreamState
pub(crate) use handlers::DreamState;

// 导入 AI 模块类型
use crate::component::memory::{MemoryManager, MemoryManagerConfig};
use crate::component::persona::dynamic_persona::ThreadSafePersona;
use crate::component::social::{DialogueClient, DialogueEventHandler};
use crate::component::social::{NarrativeGenerator, RelationshipStore};
use crate::soul::reflector::Validator;

// 重导出 context 模块的公共 API
pub use context::{
    AttributesGlimpse, ContextResponse, create_attributes_glimpse,
    generate_context_markdown_no_relationship,
};

// 重导出 cognitive_context 模块的公共 API
pub use cognitive_context::{
    AvailableActionInfo, CognitiveContext, CognitiveContextBuilder, CognitiveContextConfig,
    DecisionContext, Drive, MotivationContext, PerceptionContext, PlanningContext,
};

// ============================================================================
// 核心类型定义
// ============================================================================

#[path = "api_impl/router.rs"]
mod router;
#[path = "api_impl/server.rs"]
mod server;
#[path = "api_impl/state_factory.rs"]
mod state_factory;
#[path = "api_impl/types.rs"]
mod types;

pub use router::create_api_router;
pub use server::{get_static_serve_dir, run_http_server};
pub use state_factory::*;
pub use types::*;
