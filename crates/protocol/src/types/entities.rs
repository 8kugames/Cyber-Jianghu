//! 实体相关类型
//!
//! 包含 Agent 状态、物品、场景对象等

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

// ============================================================================
// 物品类型常量（数据驱动，禁止硬编码魔法字符串）
// ============================================================================

/// 可消耗品（食物/水/药品等）
pub const ITEM_TYPE_CONSUMABLE: &str = "consumable";
/// 武器
pub const ITEM_TYPE_WEAPON: &str = "weapon";
/// 材料
pub const ITEM_TYPE_MATERIAL: &str = "material";
/// 货币
pub const ITEM_TYPE_CURRENCY: &str = "currency";

/// 生存驱动（server 从 narratives.yaml 预计算后下发）
///
/// 当属性值落入 urgency>0 的阈值段时，server 生成对应的驱动信息。
/// agent 端直接使用，无需本地推导或硬编码。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SurvivalDrive {
    /// 触发驱动的属性名（如"satiation"）
    pub attribute: String,
    /// 驱动名称（如"寻找食物"）
    pub drive: String,
    /// 驱动原因（如"肚子饿了，需要进食"）
    pub reason: String,
    /// 紧迫程度（1-10，由 narratives.yaml 定义）
    pub urgency: u8,
    /// 目标（如"寻找食物充饥"）
    pub goal: String,
}

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

    /// 生存驱动列表（server 从 narratives.yaml 预计算）
    /// 包含紧迫属性对应的驱动名称、原因和紧迫程度
    #[serde(default)]
    pub survival_drives: Vec<SurvivalDrive>,

    /// 状态效果（如中毒、受伤等）
    #[serde(default)]
    pub status_effects: Vec<String>,

    /// 背包物品
    #[serde(default)]
    pub inventory: Vec<InventoryItem>,

    /// 已掌握的技能
    #[serde(default)]
    pub skills: Vec<SkillInfo>,

    /// 已知配方详情（Server 权威，每 tick 下发）
    #[serde(default)]
    pub recipe_details: Vec<RecipeDetail>,

    /// 当前年龄（游戏年，由 Server 从 birth_tick + time.yaml 计算）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_years: Option<u32>,

    /// 最大寿命（游戏年，由 Server 从 game_rules.yaml 下发）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age: Option<u32>,
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

    /// 便捷访问器：获取饱食度
    pub fn satiation(&self) -> i32 {
        self.get_i32("satiation").unwrap_or(0)
    }

    /// 便捷访问器：获取饱饮度
    pub fn hydration(&self) -> i32 {
        self.get_i32("hydration").unwrap_or(0)
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

    /// 物品类型（consumable/weapon/material 等）
    #[serde(default)]
    pub item_type: String,
}

/// 技能信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInfo {
    /// 技能 ID（如 martial/sword-basic）
    pub skill_id: String,
    /// 技能名称（中文）
    pub name: String,
}

/// 配方详情（Server 权威，每 tick 下发到 AgentSelfState）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecipeDetail {
    /// 配方 ID（对应 recipes.yaml 中的 key）
    pub recipe_id: String,
    /// 配方名称（显示用）
    pub name: String,
    /// 配方描述
    pub description: String,
    /// 所需材料
    pub materials: Vec<RecipeMaterialInfo>,
    /// 产出物品 ID
    pub result_item: String,
    /// 产出物品名称
    pub result_item_name: String,
    /// 产出数量
    pub result_quantity: i32,
    /// 体力消耗
    pub stamina_cost: i32,
}

/// 配方材料信息
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecipeMaterialInfo {
    /// 材料 ID
    pub item_id: String,
    /// 材料名称
    pub item_name: String,
    /// 所需数量
    pub quantity: i32,
}

/// 技能内容（用于 Server → Agent 下发 SKILL.md body）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillContent {
    /// 技能 ID（如 martial/sword-basic）
    pub skill_id: String,
    /// 技能名称（中文）
    pub name: String,
    /// SKILL.md body 内容（行为指令 markdown）
    pub body: String,
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

    /// 该角色最近的动作（供其他 Agent 观察上下文）
    #[serde(default)]
    pub recent_actions: Vec<RecentAction>,
}

/// 最近动作记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentAction {
    /// Tick 编号
    pub tick_id: i64,
    /// 动作类型
    pub action_type: String,
    /// 对话内容（如果有）
    #[serde(default)]
    pub content: Option<String>,
    /// 结果描述
    pub result: String,
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

    /// 动作名称（简短标识）
    #[serde(default)]
    pub name: String,

    /// 动作描述
    #[serde(default)]
    pub description: String,

    /// 动作分类
    #[serde(default)]
    pub category: String,

    /// 有效目标（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_targets: Option<Vec<String>>,

    /// 必需的 action_data 字段名列表（如 ["content"]、["target_agent_id", "item_id"]）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_fields: Vec<String>,

    /// 可选的 action_data 字段名列表（如 ["channel", "target_agent_id"]）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub optional_fields: Vec<String>,

    /// OOC 风险等级
    /// High → 强制 LLM 审核, Medium → 抽审, Low → 跳过 LLM
    #[serde(default)]
    pub ooc_risk: OocRisk,

    /// 动作需求列表（消耗/前置条件，从 actions.yaml 直传）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requirements: Vec<ActionRequirementInfo>,

    /// 动作效果列表（属性变化/物品变化，从 actions.yaml 直传）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<ActionEffectInfo>,
}

/// 动作的 OOC（Out-of-Character）风险等级，决定天魂分级审核策略。
/// 真闭集：值域被 websocket/types.rs:82 的 match 写死（3 分支 → always/adaptive/skip 桶）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OocRisk {
    /// 低风险，跳过审核
    Low,
    /// 中风险，自适应审核
    Medium,
    /// 高风险，必须审核
    High,
}

impl Default for OocRisk {
    fn default() -> Self {
        Self::Low
    }
}

// ============================================================================
// 动作需求与效果 — 通用数据驱动类型
// ============================================================================

/// 动作需求（通用，数据驱动）
///
/// 从 actions.yaml requirements 字段直传。
/// 通用 key-value 结构，支持任意 requirement_type 扩展。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRequirementInfo {
    /// 需求类型（"attribute" | "item" | "location" | "skill" 等，可扩展）
    pub requirement_type: String,

    /// 目标（"self" | "target"，默认 "self"）
    #[serde(default)]
    pub target: String,

    /// 通用参数（attribute/item/location 各类型的具体参数）
    #[serde(flatten)]
    pub params: HashMap<String, serde_json::Value>,
}

/// 动作效果（通用，数据驱动）
///
/// 从 actions.yaml effects 字段直传。
/// 通用 key-value 结构，支持任意 effect_type 扩展。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionEffectInfo {
    /// 效果类型（"attribute_change" | "add_item" | "remove_item" 等，可扩展）
    pub effect_type: String,

    /// 目标（"self" | "target"，默认 "self"）
    #[serde(default)]
    pub target: String,

    /// 通用参数
    #[serde(flatten)]
    pub params: HashMap<String, serde_json::Value>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ooc_risk_serde_roundtrip() {
        // 序列化
        assert_eq!(serde_json::to_string(&OocRisk::High).unwrap(), "\"high\"");
        assert_eq!(
            serde_json::to_string(&OocRisk::Medium).unwrap(),
            "\"medium\""
        );
        assert_eq!(serde_json::to_string(&OocRisk::Low).unwrap(), "\"low\"");

        // 反序列化
        assert_eq!(
            serde_json::from_str::<OocRisk>("\"high\"").unwrap(),
            OocRisk::High
        );
        assert_eq!(
            serde_json::from_str::<OocRisk>("\"medium\"").unwrap(),
            OocRisk::Medium
        );
        assert_eq!(
            serde_json::from_str::<OocRisk>("\"low\"").unwrap(),
            OocRisk::Low
        );

        // 默认值 = Low
        assert_eq!(OocRisk::default(), OocRisk::Low);
    }
}
