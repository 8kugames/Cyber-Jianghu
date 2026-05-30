// ============================================================================
// HTTP API Handlers - 所有 API 端点的处理器
// ============================================================================
//
// 按功能域拆分为独立子模块，每个子模块负责一组相关端点。
// 所有公共 handler 函数通过 pub use 重导出，保持外部路径不变。

mod basic;
mod biography;
mod character_helpers;
mod character_info;
mod character_register;
mod config;
mod discovery;
mod lifespan;
mod llm_config;
mod memory;
mod multi_character;
mod relationship;
mod soul_cycle;
mod tick_notify;
mod validate;

// Re-export parent module items for sub-module access via `super::xxx`
pub(super) use super::HttpApiState;
pub(super) use super::cognitive_context;
pub(super) use super::context;
pub(super) use super::dto;
pub(super) use super::service;
pub(super) use super::soul_cycle_recorder;

// Re-export all public items from sub-modules
pub(crate) use basic::*;
pub(crate) use biography::*;
pub(crate) use character_info::*;
pub(crate) use character_register::*;
pub(crate) use config::*;
pub(crate) use discovery::*;
pub(crate) use lifespan::*;
pub(crate) use llm_config::*;
pub(crate) use memory::*;
pub(crate) use multi_character::*;
pub(crate) use relationship::*;
pub(crate) use soul_cycle::*;
pub(crate) use tick_notify::*;
pub(crate) use validate::*;
