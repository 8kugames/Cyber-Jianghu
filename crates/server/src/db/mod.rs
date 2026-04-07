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
mod common;
mod ground_item_ops;
mod item_ops;
mod state_ops;

// 导出公共API - 连接池初始化和工具函数
pub use common::init_db_pool;

// 导出公共API - Agent操作
pub use agent_ops::{
    DeviceConnectResult, RebirthResult, connect_device, get_agent_by_device_id, get_agent_by_id,
    get_all_agents, get_intent_timeout_stats, rebirth_agent, register_agent_transactional,
    update_agent_location, update_agent_online, update_device_last_seen, verify_device_token,
};

// 导出公共API - AgentState操作
pub use state_ops::{
    batch_insert_agent_states, get_all_alive_agents_latest_states, get_current_world_tick_id,
    get_last_tick_time, get_latest_agent_state, get_latest_state_tick_id,
};

// 导出公共API - Tick日志操作
pub use state_ops::{create_tick_log, update_tick_log};

// 导出公共API - Agent动作日志操作
pub use state_ops::batch_insert_action_logs;

// 导出公共API - 地面物品操作
pub use ground_item_ops::{add_ground_item, get_ground_items_by_node, get_ground_items_by_nodes, remove_ground_item};

// 导出公共API - 物品操作
pub use item_ops::sync_items_from_config;

// 数据库连接池类型别名
pub type DbPool = sqlx::PgPool;
