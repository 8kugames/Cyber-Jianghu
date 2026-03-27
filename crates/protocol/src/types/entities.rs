//! 实体相关类型
//!
//! 包含 Agent 状态、物品、场景对象等

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Agent 自身状态（完全动态架构）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSelfState {
    /// 动态属性映射（完全数据驱动）
    /// 所有属性从配置文件定义，支持任意扩展
    /// 状态值（HP、体力等）均为 i32 类型
    #[serde(default)]
    pub attributes: HashMap<String, i32>,

    /// 派生属性映射（实时计算，浮点数）
    /// 如闪避率、暴击率等，基于公式实时计算
    #[serde(default)]
    pub derived_attributes: HashMap<String, f32>,

    /// 属性叙事描述（数据驱动）
    /// 将数值转换为自然语言描述，便于 LLM 理解
    #[serde(default)]
    pub attribute_descriptions: HashMap<String, String>,

    /// 状态效果（如中毒、受伤等）
    #[serde(default)]
    pub status_effects: Vec<String>,

    /// 背包物品
    #[serde(default)]
    pub inventory: Vec<InventoryItem>,
}

impl AgentSelfState {
    /// 便捷访问器：获取整数属性
    pub fn get_i32(&self, name: &str) -> Option<i32> {
        self.attributes.get(name).copied()
    }

    /// 便捷访问器：获取 HP
    pub fn hp(&self) -> i32 {
        self.get_i32("hp").unwrap_or(0)
    }

    /// 便捷访问器：获取体力
    pub fn stamina(&self) -> i32 {
        self.get_i32("stamina").unwrap_or(100)
    }

    /// 便捷访问器：获取饥饿值
    pub fn hunger(&self) -> i32 {
        self.get_i32("hunger").unwrap_or(0)
    }

    /// 便捷访问器：获取口渴值
    pub fn thirst(&self) -> i32 {
        self.get_i32("thirst").unwrap_or(0)
    }
}

/// 背包物品
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InventoryItem {
    /// 物品 ID
    pub item_id: String,

    /// 物品名称
    pub name: String,

    /// 数量
    pub quantity: i32,

    /// 是否已装备
    #[serde(default)]
    pub is_equipped: bool,
}

/// 周围实体（其他 Agent）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    /// Agent ID
    pub id: Uuid,

    /// Agent 名称
    pub name: String,

    /// 距离（MVP 阶段固定为 0，同一节点）
    #[serde(default)]
    pub distance: i32,

    /// 状态（存活、死亡等）
    pub state: String,

    /// 是否敌对
    #[serde(default)]
    pub hostile: bool,
}

/// 场景物品（可拾取）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneItem {
    /// 物品 ID
    pub item_id: String,

    /// 物品名称
    pub name: String,

    /// 数量
    pub quantity: i32,

    /// 物品类型（食物、水、武器等）
    #[serde(default)]
    pub item_type: String,
}

/// 可用动作
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailableAction {
    /// 动作类型
    pub action: String,

    /// 动作描述
    #[serde(default)]
    pub description: String,

    /// 有效目标（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_targets: Option<Vec<String>>,
}

/// 初始物品配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitialItem {
    /// 物品 ID
    pub item_id: String,

    /// 物品名称
    pub name: String,

    /// 数量
    pub quantity: i32,

    /// 物品描述
    pub description: String,
}

/// 死亡信息（用于内部传递）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeathInfo {
    /// 死亡原因代码
    pub cause: String,
    /// 死亡描述
    pub message: String,
}
