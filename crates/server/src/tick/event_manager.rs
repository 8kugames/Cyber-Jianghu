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
//
// 共享方式：
// - TickScheduler 和 IntentWorker 共享同一个 EventManager
// - 使用 Arc<Mutex<EventManager>> 确保线程安全
// - EventManager 内部 events 字段使用 std::sync::Mutex<HashMap> 保护
// ============================================================================

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use crate::models::WorldEvent;

/// 事件管理器（可共享）
pub type SharedEventManager = Arc<Mutex<EventManager>>;

/// 事件管理器
///
/// 负责管理Tick期间的所有事件
pub struct EventManager {
    /// 当前Tick的事件记录（按 Agent ID 分组）
    events: Mutex<HashMap<Uuid, Vec<WorldEvent>>>,
}

impl EventManager {
    /// 创建新的事件管理器
    pub fn new() -> Self {
        Self {
            events: Mutex::new(HashMap::new()),
        }
    }

    /// 创建可共享的 EventManager
    pub fn new_shared() -> SharedEventManager {
        Arc::new(Mutex::new(Self::new()))
    }

    /// 为指定 Agent 添加事件
    pub fn add_event_for_agent(&self, agent_id: Uuid, event: WorldEvent) {
        let mut guard = self.events.lock().unwrap();
        guard.entry(agent_id).or_default().push(event);
    }

    /// 获取 Agent 的本 Tick 事件列表
    pub fn get_events_for_agent(&self, agent_id: Uuid) -> Vec<WorldEvent> {
        let guard = self.events.lock().unwrap();
        guard.get(&agent_id).cloned().unwrap_or_default()
    }

    /// 清空本 Tick 的所有事件
    pub fn clear(&self) {
        let mut guard = self.events.lock().unwrap();
        guard.clear();
    }
}

impl Default for EventManager {
    fn default() -> Self {
        Self::new()
    }
}
