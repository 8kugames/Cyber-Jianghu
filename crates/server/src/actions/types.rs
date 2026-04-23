// ============================================================================
// 动作类型定义
// ============================================================================
//
// 定义动作执行结果、状态变更等核心类型
// ============================================================================

use cyber_jianghu_protocol::AttributeValue;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ============================================================================
// 动作执行结果
// ============================================================================

/// 动作执行结果
///
/// 记录动作执行的完整结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionExecutionResult {
    /// 对应的 Intent ID（如果有）
    pub intent_id: Option<Uuid>,

    /// 是否成功
    pub success: bool,

    /// 结果消息
    pub message: String,

    /// 状态变更列表
    pub state_changes: Vec<StateChange>,

    /// 动作类型（用于日志）
    pub action_type: String,
}

impl ActionExecutionResult {
    /// 创建成功结果
    pub fn success(
        message: impl Into<String>,
        action_type: impl Into<String>,
        intent_id: Option<Uuid>,
    ) -> Self {
        Self {
            intent_id,
            success: true,
            message: message.into(),
            state_changes: Vec::new(),
            action_type: action_type.into(),
        }
    }

    /// 创建失败结果
    pub fn failure(
        message: impl Into<String>,
        action_type: impl Into<String>,
        intent_id: Option<Uuid>,
    ) -> Self {
        Self {
            intent_id,
            success: false,
            message: message.into(),
            state_changes: Vec::new(),
            action_type: action_type.into(),
        }
    }

    /// 添加状态变更
    pub fn add_change(&mut self, change: StateChange) {
        self.state_changes.push(change);
    }
}

// ============================================================================
// 状态变更
// ============================================================================

/// 物品效果定义（用于 StateChange）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemEffect {
    /// 目标属性
    pub attribute: String,
    /// 操作符（add/set/multiply）
    pub operator: String,
    /// 效果值
    pub value: i32,
}

/// 状态变更
///
/// 记录动作执行后产生的状态变化
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StateChange {
    /// 通用属性变化
    AttributeChanged {
        /// Agent ID
        agent_id: Uuid,
        /// 属性名
        attribute: String,
        /// 变化值
        delta: AttributeValue,
    },

    /// HP 变化
    HpChanged {
        /// Agent ID
        agent_id: Uuid,
        /// 变化值（正数为增加，负数为减少）
        delta: i32,
    },

    /// 饥饿值变化
    HungerChanged {
        /// Agent ID
        agent_id: Uuid,
        /// 变化值
        delta: i32,
    },

    /// 口渴值变化
    ThirstChanged {
        /// Agent ID
        agent_id: Uuid,
        /// 变化值
        delta: i32,
    },

    /// 体力值变化
    StaminaChanged {
        /// Agent ID
        agent_id: Uuid,
        /// 变化值
        delta: i32,
    },

    /// 物品转移
    ItemTransferred {
        /// 来源 Agent ID
        from: Uuid,
        /// 目标 Agent ID
        to: Uuid,
        /// 物品 ID
        item_id: String,
        /// 数量
        quantity: i32,
    },

    /// 物品使用（消耗）
    ItemUsed {
        /// Agent ID
        agent_id: Uuid,
        /// 物品 ID
        item_id: String,
        /// 物品效果（扣除物品成功后应用）
        effects: Vec<ItemEffect>,
    },

    /// 武器装备
    ItemEquipped {
        /// Agent ID
        agent_id: Uuid,
        /// 物品 ID
        item_id: String,
    },

    /// 对话消息
    MessageSpoken {
        /// Agent ID
        agent_id: Uuid,
        /// 对话内容
        content: String,
        /// 目标 Agent ID（None 表示向在场所有人说）
        target_agent_id: Option<Uuid>,
        already_broadcast: bool,
    },

    /// Agent 死亡
    AgentDied {
        /// Agent ID
        agent_id: Uuid,
        /// 死亡原因
        cause: String,
    },

    /// 物品掉落
    ItemDropped {
        /// 原拥有者 Agent ID
        from_agent: Uuid,
        /// 物品 ID
        item_id: String,
        /// 数量
        quantity: i32,
        /// 掉落地点（节点 ID）
        location: String,
    },

    /// 物品拾取（添加到背包）
    ItemPickedUp {
        /// Agent ID
        agent_id: Uuid,
        /// 物品 ID
        item_id: String,
        /// 数量
        quantity: i32,
    },

    /// 采集获得物品
    ItemGathered {
        /// Agent ID
        agent_id: Uuid,
        /// 物品 ID
        item_id: String,
        /// 数量
        quantity: i32,
    },

    /// 制造获得物品
    ItemCrafted {
        /// Agent ID
        agent_id: Uuid,
        /// 物品 ID
        item_id: String,
        /// 数量
        quantity: i32,
    },

    /// 位置变更（移动到新的子场景）
    LocationChanged {
        /// Agent ID
        agent_id: Uuid,
        /// 旧位置
        old_location: String,
        /// 新位置
        new_location: String,
    },

    /// 技能习得
    SkillLearned {
        /// Agent ID
        agent_id: Uuid,
        /// 技能 ID（如 martial/sword-basic）
        skill_id: String,
    },
}
