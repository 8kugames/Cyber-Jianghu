// ============================================================================
// OpenClaw Cyber-Jianghu MVP 动作系统模块
// ============================================================================
//
// 本模块实现数据驱动的动作系统，支持多种动作类型：
//
// 基础动作：
// - idle: 无操作，保底指令
// - speak: 对话，传递信息，所有人可见
// - move: 移动到目标位置
//
// 物品相关动作：
// - use: 使用物品（消耗品或装备），如吃馒头、喝水、装备武器
// - pickup: 从地面拾取物品
// - drop: 将物品丢弃到地面
// - give: 给予物品，单向转移所有权
// - steal: 偷窃，从目标 Agent 背包中偷取物品（有成功率）
// - trade: 交易，带价格协商的物品转移（原子操作）
//
// 战斗动作：
// - attack: 攻击目标，造成伤害
//
// 生产动作：
// - gather: 采集资源
// - craft: 制造物品（原子操作，配方材料扣除+产物添加）
//
// 设计原则：
// 1. 完全数据驱动：动作参数从配置文件读取
// 2. 验证-执行分离：validator 负责验证，executor 负责执行
// 3. 状态变更原子化：使用事务确保多步操作的原子性
// 4. 详细的错误处理：失败时返回明确错误而非静默回退
// ============================================================================

mod executor;
mod types;
mod validator;

pub use executor::ActionExecutor;
pub use types::{ActionExecutionResult, ItemEffect, StateChange};
pub use validator::validate_action;

// ============================================================================
// 动作数据结构（从 Intent 的 action_data 解析）
// ============================================================================

use serde::{Deserialize, Serialize};

/// speak 动作数据
///
/// 对话内容所有人可见
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeakData {
    /// 对话内容
    pub content: String,
    /// 目标 Agent ID（None 表示向在场所有人说）
    #[serde(default)]
    pub target_agent_id: Option<uuid::Uuid>,
}

/// move 动作数据
///
/// 移动到目标位置（Phase 1 暂无效果）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveData {
    /// 目标位置 ID
    pub target_location: String,
}

/// give 动作数据
///
/// 给予物品，单向转移所有权
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GiveData {
    /// 目标 Agent ID
    pub target_agent_id: String,
    /// 物品 ID
    pub item_id: String,
    /// 数量
    pub quantity: i32,
}

/// steal 动作数据
///
/// 偷窃，从目标 Agent 背包中偷取物品
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StealData {
    /// 目标 Agent ID
    pub target_agent_id: String,
    /// 物品 ID
    pub item_id: String,
}

/// use 动作数据
///
/// 使用物品（消耗品或装备武器）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UseData {
    /// 物品 ID
    pub item_id: String,
}

/// pickup 动作数据
///
/// 从场景中拾取物品
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PickupData {
    /// 物品 ID
    pub item_id: String,
    #[serde(default = "default_quantity")]
    pub quantity: i32,
}

fn default_quantity() -> i32 {
    1
}

/// drop 动作数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropData {
    pub item_id: String,
    #[serde(default = "default_quantity")]
    pub quantity: i32,
}

/// gather 动作数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatherData {
    pub target_id: String,
}

/// craft 动作数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CraftData {
    pub recipe_id: String,
}

/// attack 动作数据
///
/// 攻击目标 Agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttackData {
    /// 目标 Agent ID
    pub target_agent_id: String,
}

/// trade 动作数据
///
/// 交易，带价格协商的物品转移
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeData {
    /// 目标 Agent ID
    pub target_agent_id: String,
    /// 物品 ID
    pub item_id: String,
    /// 物品数量
    pub quantity: i32,
    /// 价格（单位：两银子）
    pub price: i32,
}

/// shout 动作数据
///
/// 大喊，内容对当前位置所有人可见
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShoutData {
    /// 喊叫内容
    pub content: String,
}

/// flee 动作数据
///
/// 逃跑，移动到相邻位置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleeData {
    /// 逃跑目标位置 ID
    pub target_location: String,
}

// ============================================================================
// 配置访问说明
// ============================================================================
//
// 本模块已实现完全数据驱动架构。
// 所有 action 参数通过 ActionRegistry 访问，server 代码不预设任何 action 类型。
//
// 使用示例：
// ```rust
// use crate::game_data::{ActionRegistry, ActionField};
//
// // 获取基础伤害值
// let damage = ActionRegistry::get_i32("攻击", ActionField::BaseDamage)
//     .unwrap_or(0);  // 配置缺失时返回 0
//
// // 获取成功率
// let success_rate = ActionRegistry::get_f32("偷窃", ActionField::SuccessRate)
//     .unwrap_or(0.0);  // 配置缺失时返回 0.0
// ```
//
// 注意：如果配置文件中缺少必要参数，应返回失败结果而非使用硬编码默认值。
// ============================================================================
