// ============================================================================
// OpenClaw Cyber-Jianghu MVP 数据库模块
// ============================================================================
//
// 本模块包含数据库相关的所有功能，包括：
// - 数据库连接池管理
// - Agent相关数据库操作
// - AgentState相关数据库操作
// - Tick日志相关数据库操作
//
// 模块结构：
// - common: 数据库连接池初始化和共享工具
// - agent_ops: Agent CRUD操作
// - state_ops: AgentState和日志操作
// - ground_item_ops: 地面物品操作
//
// 使用技术：
// - SQLx: Rust异步数据库库
// - PostgreSQL: 数据库
//
// 设计原则：
// 1. 使用连接池管理数据库连接
// 2. 批量操作优化性能
// 3. 事务保证数据一致性
// 4. 清晰的错误处理
// ============================================================================

// 公共模块
mod agent_ops;
mod audit_ops;
mod common;
mod ground_item_ops;
mod item_ops;
mod role_ops;
mod state_ops;
mod vendor_ops;

// 导出公共API - 连接池初始化和工具函数
pub use common::{
    DbRuntimeHealth, DbRuntimeHealthState, create_db_runtime_health_state, init_db_pool,
    record_db_probe_result, start_db_health_probe,
};

// 导出公共API - Agent操作
pub use agent_ops::{
    AutoRebirthParams, AutoRebirthResult, DeviceConnectResult, RetireResult, assign_initial_recipes,
    auto_rebirth_agent, batch_get_known_recipe_ids, connect_device, ensure_old_agent_id_not_nil,
    find_device_by_auth_token, get_agent_by_device_id, get_agent_by_id, get_all_agents,
    get_device_token, get_intent_timeout_stats, get_known_recipe_ids, record_recipe_observation,
    register_agent_transactional, register_device, retire_agent, rotate_device_token,
    update_agent_biography, update_agent_location, update_agent_online, update_device_last_seen,
    verify_device_strict, verify_device_token,
};

pub use audit_ops::{AuditLogEntry, build_audit_request_context, insert_audit_log};

// 导出公共API - AgentState操作
pub use state_ops::{
    batch_insert_agent_states, get_all_alive_agents_latest_states, get_current_world_tick_id,
    get_last_tick_time, get_latest_agent_state, get_latest_state_tick_id,
    get_or_init_deployment_time, upsert_agent_state, upsert_agent_state_in_tx,
};

// 导出公共API - Tick日志操作
pub use state_ops::{create_tick_log, update_tick_log};

// 导出公共API - Agent动作日志操作
pub use state_ops::{
    batch_insert_action_logs, update_soul_cycle_metadata, upsert_agent_daily_summary,
};

// 导出公共API - Agent每日摘要查询
pub use state_ops::{
    AgentDailySummary, count_agent_daily_summaries, get_agent_daily_summaries_by_agent,
    list_agent_daily_summaries,
};

// 导出公共API - 涌现：跨 tick 动作观察
pub use state_ops::get_recent_actions_batch;

// 导出公共API - Agent 每日动作统计
pub use state_ops::{AgentDailyActionStats, get_agent_daily_action_stats};

// 导出公共API - 地面物品操作
pub use ground_item_ops::{
    add_ground_item, get_ground_items_by_node, get_ground_items_by_nodes, remove_ground_item,
};

// 导出公共API - 物品操作
pub use item_ops::sync_items_from_config;

// 导出公共API - Vendor 补货操作
pub use vendor_ops::{
    VendorRefillRule, get_all_enabled_vendor_refills, get_vendor_refills, remove_vendor_refill,
    set_vendor_refill, toggle_vendor_refill,
};

// 导出公共API - 角色身份操作
pub use role_ops::{AgentRole, assign_role, get_agent_roles, remove_role};

// 数据库连接池类型别名
pub type DbPool = sqlx::PgPool;
