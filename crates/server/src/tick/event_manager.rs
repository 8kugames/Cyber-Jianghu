// ============================================================================
// OpenClaw Cyber-Jianghu MVP Event Manager
// ============================================================================
//
// 事件管理器负责Tick期间的事件创建和管理，包括：
// 1. 创建WorldEvent
// 2. 按Agent ID分组管理事件
// 3. 查询和清理事件
//
// 设计原则：
// 1. 简单的内存存储（Tick结束后清空）
// 2. 按Agent ID分组，方便查询
// 3. 支持单个和批量事件操作
// ============================================================================

use std::collections::HashMap;
use uuid::Uuid;

use crate::models::WorldEvent;

/// 事件管理器
///
/// 负责管理Tick期间的所有事件
pub struct EventManager {
    /// 当前Tick的事件记录（按 Agent ID 分组）
    events: HashMap<Uuid, Vec<WorldEvent>>,
}

impl EventManager {
    /// 创建新的事件管理器
    pub fn new() -> Self {
        Self {
            events: HashMap::new(),
        }
    }

    /// 为指定 Agent 添加事件
    pub fn add_event_for_agent(&mut self, agent_id: Uuid, event: WorldEvent) {
        self.events
            .entry(agent_id)
            .or_default()
            .push(event);
    }

    /// 获取 Agent 的本 Tick 事件列表
    pub fn get_events_for_agent(&self, agent_id: Uuid) -> Vec<WorldEvent> {
        self.events.get(&agent_id).cloned().unwrap_or_default()
    }

    /// 清空本 Tick 的所有事件
    pub fn clear(&mut self) {
        self.events.clear();
    }
}

impl Default for EventManager {
    fn default() -> Self {
        Self::new()
    }
}
