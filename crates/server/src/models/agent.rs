// ============================================================================
// Agent 相关数据结构
// ============================================================================

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

/// Agent基本信息
///
/// 存储Agent的基本信息，包括名称、人设Prompt等
/// 认证信息存储在 devices 表，通过 device_id 关联
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Agent {
    /// Agent唯一ID（UUID）
    pub agent_id: Uuid,

    /// 所属设备ID（关联 devices 表）
    pub device_id: Uuid,

    /// Agent名称（如：老板娘、富商、刀客、新秀、小偷）
    pub name: String,

    /// Agent人设Prompt（LLM使用）
    /// 定义Agent的性格、行为规则等
    pub system_prompt: String,

    /// Agent 状态（active/retired/dead）
    pub status: String,

    /// 归隐时间（转生时设置）
    pub retired_at: Option<DateTime<Utc>>,

    /// 创建时间
    pub created_at: DateTime<Utc>,

    /// 最后一次上报意图的时间
    pub last_tick_online: Option<DateTime<Utc>>,

    /// 角色出生 tick（秒级时间戳，不可变）
    /// NULL = 不朽（迁移前角色不受寿命约束）
    pub birth_tick: Option<i64>,

    /// 角色注册时上报的 LLM 模型 ID（如 glm-4、gpt-4o）
    /// NULL = 旧数据 / 未上报（兼容存量）
    #[serde(default)]
    pub model_id: Option<String>,
}

/// Agent状态
///
/// 每Tick记录一次Agent的状态快照
/// 使用 COI 架构：组件组合代替 HashMap 扁平结构
/// - primary_attributes: 先天属性组件（力量、敏捷、体质等）
/// - status: 状态值组件（HP、体力、饥饿、口渴等）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    /// 状态记录ID
    pub id: i64,

    /// Agent ID
    pub agent_id: Uuid,

    /// Agent 名称（从 agents 表 JOIN 填充，用于事件描述等）
    #[serde(default)]
    pub name: String,

    /// Tick编号（递增）
    pub tick_id: i64,

    /// 同一 `(agent_id, tick_id)` 行内的乐观锁版本号
    pub state_version: i64,

    /// 先天属性组件（力量、敏捷、体质、智力、魅力、福缘）
    pub primary_attributes: crate::game_data::types::AttributeComponent,

    /// 状态值组件（HP、体力、饥饿、口渴、内力、理智、声望、银两）
    pub status: crate::game_data::types::StatusComponent,

    /// 当前所在节点ID
    pub node_id: String,

    /// 是否存活
    pub is_alive: bool,

    /// 本Tick内是否已清空过背包（防止重复清空）
    pub inventory_cleared_this_tick: bool,

    /// 已掌握的 LLM 行为指令 ID 列表（对应 SKILL.md 文件）
    ///
    /// 持久化到 DB 的 JSONB attributes._skills 字段。
    /// 通过 WorldState.skills 下发给 Agent，Agent 据此加载对应 SKILL.md 到 prompt。
    /// 不是 RPG 技能列表，无任何数值属性关联。
    /// 详见 tick/processor/skill_mutator.rs。
    #[serde(default)]
    pub skills: Vec<String>,

    /// action category 成功执行计数（用于技能习得阈值判定）
    ///
    /// 持久化到 DB 的 JSONB attributes._action_counts 字段。
    /// 不广播到 WorldState（仅 Server 内部使用）。
    /// key: action category（如 "social", "martial"）
    /// value: 该 category 的累计成功执行次数
    #[serde(default)]
    pub action_counts: std::collections::HashMap<String, i32>,

    /// 角色出生 tick（秒级时间戳，不可变）
    /// 从 agents 表 JOIN 获取，缓存到 DashMap
    /// NULL = 不朽（迁移前角色不受寿命约束）
    #[serde(default)]
    pub birth_tick: Option<i64>,

    /// 衰减小数累计器（运行时状态，不持久化）
    ///
    /// 解决 `decay_per_tick < 1.0` 时 f32→i32 截断为 0 的问题。
    /// 每 tick 把 `-decay_amount * season_modifier` 累加到对应属性上，
    /// 当 |accumulator| ≥ 1.0 时扣减整数，余数保留。
    #[serde(skip, default)]
    pub decay_accumulator: std::collections::HashMap<String, f32>,

    /// 状态记录时间
    pub created_at: DateTime<Utc>,
}
