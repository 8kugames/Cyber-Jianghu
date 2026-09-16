// ============================================================================
// OpenClaw Cyber-Jianghu MVP Agent数据库操作模块
// ============================================================================
//
// 本模块实现Agent相关的数据库操作，包括：
// - 创建Agent
// - 查询Agent（by ID, by token, all）
// - 更新Agent状态（在线时间、位置）

use anyhow::{Context, Result};
use sqlx::{PgPool, Postgres};
use std::collections::HashMap;
use tracing::{debug, error, info};
use uuid::Uuid;

use crate::models::{Agent, AgentState};

use super::common::generate_secure_token;

mod agent_crud;
mod device;
mod lifecycle;
mod recipes;
mod token;

pub use agent_crud::*;
pub use device::*;
pub use lifecycle::*;
pub use recipes::*;
pub use token::*;
