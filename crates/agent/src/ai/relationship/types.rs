// ============================================================================
// 关系记忆类型定义
// ============================================================================
//
// 定义关系记忆的数据结构：
// - KeyEvent：关键事件记录
// - RelationshipMemory：对其他 Agent 的关系记忆
//
// 设计原则：
// 1. 关系完全本地化，服务端无法访问
// 2. 支持好感度追踪和关键事件记录
// 3. 为 LLM 提供结构化的关系上下文
// ============================================================================

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 关键事件
///
/// 记录与目标 Agent 互动的关键事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyEvent {
    /// Tick ID
    pub tick_id: i64,
    /// 事件类型（如：对话、交易、攻击、帮助）
    pub event_type: String,
    /// 事件描述
    pub description: String,
    /// 好感度变化
    pub favorability_delta: i32,
    /// 事件时间戳
    pub timestamp: DateTime<Utc>,
}

impl KeyEvent {
    /// 创建新的关键事件
    pub fn new(
        tick_id: i64,
        event_type: impl Into<String>,
        description: impl Into<String>,
        favorability_delta: i32,
    ) -> Self {
        Self {
            tick_id,
            event_type: event_type.into(),
            description: description.into(),
            favorability_delta,
            timestamp: Utc::now(),
        }
    }
}

/// 关系记忆
///
/// 存储对某个目标 Agent 的关系记忆
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipMemory {
    /// 目标 Agent ID
    pub target_agent_id: Uuid,
    /// 目标 Agent 名称
    pub target_name: String,
    /// 好感度（-100 到 100，0 为中性）
    pub favorability: i32,
    /// 关键事件列表
    pub key_events: Vec<KeyEvent>,
    /// 最后交互的 Tick ID
    pub last_interaction_tick: i64,
    /// 最后更新时间
    pub updated_at: DateTime<Utc>,
    /// AI 自主生成的好感度叙事化描述（20字以内）
    pub self_description: String,
    /// 描述生成时的 Tick ID
    pub description_tick: i64,
}

impl RelationshipMemory {
    /// 创建新的关系记忆
    pub fn new(target_agent_id: Uuid, target_name: impl Into<String>) -> Self {
        Self {
            target_agent_id,
            target_name: target_name.into(),
            favorability: 0,
            key_events: Vec::new(),
            last_interaction_tick: 0,
            updated_at: Utc::now(),
            self_description: String::new(),
            description_tick: 0,
        }
    }

    /// 更新好感度
    ///
    /// 好感度会被限制在 -100 到 100 之间
    pub fn update_favorability(&mut self, delta: i32) {
        self.favorability = (self.favorability + delta).clamp(-100, 100);
        self.updated_at = Utc::now();
    }

    /// 设置好感度（绝对值）
    ///
    /// 好感度会被限制在 -100 到 100 之间
    pub fn set_favorability(&mut self, value: i32) {
        self.favorability = value.clamp(-100, 100);
        self.updated_at = Utc::now();
    }

    /// 添加关键事件
    ///
    /// 最多保留 20 个关键事件
    pub fn add_event(&mut self, event: KeyEvent) {
        self.key_events.push(event);
        // 只保留最近的 20 个事件
        if self.key_events.len() > 20 {
            self.key_events.remove(0);
        }
        self.updated_at = Utc::now();
    }

    /// 更新最后交互时间
    pub fn update_interaction(&mut self, tick_id: i64) {
        self.last_interaction_tick = tick_id;
        self.updated_at = Utc::now();
    }

    /// 获取关系描述
    ///
    /// 根据好感度返回关系描述
    pub fn get_relationship_description(&self) -> &str {
        match self.favorability {
            i if i >= 80 => "至交好友",
            i if i >= 50 => "好友",
            i if i >= 20 => "熟人",
            i if i >= -20 => "陌生人",
            i if i >= -50 => "不喜欢",
            i if i >= -80 => "敌对",
            _ => "死敌",
        }
    }

    /// 构建 LLM 上下文
    ///
    /// 将关系记忆转换为 LLM 可以理解的文本
    pub fn build_context(&self) -> String {
        let mut context = format!(
            "与 {} 的关系：{}（好感度：{}）",
            self.target_name,
            self.get_relationship_description(),
            self.favorability
        );

        if !self.key_events.is_empty() {
            context.push_str("\n关键事件：\n");
            for (i, event) in self.key_events.iter().enumerate() {
                context.push_str(&format!(
                    "  {}. [Tick {}] {}（{}）",
                    i + 1,
                    event.tick_id,
                    event.description,
                    if event.favorability_delta > 0 {
                        format!("好感度 +{}", event.favorability_delta)
                    } else if event.favorability_delta < 0 {
                        format!("好感度 {}", event.favorability_delta)
                    } else {
                        "好感度无变化".to_string()
                    }
                ));
            }
        }

        context
    }

    /// 获取最近的事件
    pub fn get_recent_events(&self, limit: usize) -> Vec<&KeyEvent> {
        let start = if self.key_events.len() > limit {
            self.key_events.len() - limit
        } else {
            0
        };
        self.key_events[start..].iter().collect()
    }

    /// 计算好感度趋势
    ///
    /// 返回最近 N 个事件的好感度变化总和
    pub fn compute_favorability_trend(&self, recent_count: usize) -> i32 {
        self.get_recent_events(recent_count)
            .iter()
            .map(|e| e.favorability_delta)
            .sum()
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_relationship_memory_creation() {
        let target_id = Uuid::new_v4();
        let memory = RelationshipMemory::new(target_id, "张三");

        assert_eq!(memory.target_agent_id, target_id);
        assert_eq!(memory.target_name, "张三");
        assert_eq!(memory.favorability, 0);
        assert!(memory.key_events.is_empty());
        assert_eq!(memory.last_interaction_tick, 0);
    }

    #[test]
    fn test_favorability_clamping() {
        let mut memory = RelationshipMemory::new(Uuid::new_v4(), "测试");

        // 测试上限
        memory.update_favorability(150);
        assert_eq!(memory.favorability, 100);

        // 重置
        memory.set_favorability(0);

        // 测试下限
        memory.update_favorability(-150);
        assert_eq!(memory.favorability, -100);
    }

    #[test]
    fn test_event_limit() {
        let mut memory = RelationshipMemory::new(Uuid::new_v4(), "测试");

        // 添加 25 个事件
        for i in 1..=25 {
            memory.add_event(KeyEvent::new(i, "测试", format!("事件 {}", i), 0));
        }

        // 应该只保留最近的 20 个
        assert_eq!(memory.key_events.len(), 20);
        assert_eq!(memory.key_events[0].tick_id, 6); // 第 6 个事件
        assert_eq!(memory.key_events[19].tick_id, 25); // 第 25 个事件
    }

    #[test]
    fn test_relationship_description() {
        let mut memory = RelationshipMemory::new(Uuid::new_v4(), "测试");

        memory.set_favorability(90);
        assert_eq!(memory.get_relationship_description(), "至交好友");

        memory.set_favorability(60);
        assert_eq!(memory.get_relationship_description(), "好友");

        memory.set_favorability(-60);
        assert_eq!(memory.get_relationship_description(), "敌对");
    }

    #[test]
    fn test_build_context() {
        let mut memory = RelationshipMemory::new(Uuid::new_v4(), "张三");
        memory.set_favorability(50);

        memory.add_event(KeyEvent::new(1, "对话", "聊得很开心", 10));

        let context = memory.build_context();
        assert!(context.contains("张三"));
        assert!(context.contains("好友"));
        assert!(context.contains("好感度：50"));
        assert!(context.contains("聊得很开心"));
    }

    #[test]
    fn test_favorability_trend() {
        let mut memory = RelationshipMemory::new(Uuid::new_v4(), "测试");

        // 添加一些事件
        memory.add_event(KeyEvent::new(1, "测试", "事件1", 10));
        memory.add_event(KeyEvent::new(2, "测试", "事件2", -5));
        memory.add_event(KeyEvent::new(3, "测试", "事件3", 15));

        // 最近 3 个事件的总变化
        let trend = memory.compute_favorability_trend(3);
        assert_eq!(trend, 20);
    }
}
