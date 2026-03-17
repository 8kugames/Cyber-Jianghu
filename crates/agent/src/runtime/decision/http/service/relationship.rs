// ============================================================================
// Relationship Service - 关系业务逻辑
// ============================================================================
//
// 从 handlers.rs 提取的关系相关业务逻辑

use anyhow::Result;
use uuid::Uuid;

use crate::ai::relationship::{RelationshipStore, RelationshipMemory, KeyEvent};

/// 关系服务
pub struct RelationshipService<'a> {
    store: &'a RelationshipStore,
}

impl<'a> RelationshipService<'a> {
    /// 创建新的关系服务实例
    pub fn new(store: &'a RelationshipStore) -> Self {
        Self { store }
    }

    /// 获取所有关系
    pub fn get_all(&self) -> Result<Vec<RelationshipMemory>> {
        self.store.get_all_relationships()
    }

    /// 获取特定关系
    pub fn get(&self, target_id: Uuid) -> Result<Option<RelationshipMemory>> {
        self.store.get_relationship(target_id)
    }

    /// 更新关系
    ///
    /// # 参数
    /// - `target_id`: 目标 Agent UUID
    /// - `target_name`: 目标名称（创建新关系时使用）
    /// - `favorability_delta`: 好感度变化（可选）
    /// - `event`: 关键事件（可选），格式：(type, description, delta, tick_id)
    ///
    /// # 返回
    /// 更新后的关系记忆
    pub fn update(
        &self,
        target_id: Uuid,
        target_name: &str,
        favorability_delta: Option<i32>,
        event: Option<(String, String, i32, i64)>,
    ) -> Result<RelationshipMemory> {
        // 获取现有关系或创建新的
        let mut memory = match self.store.get_relationship(target_id)? {
            Some(m) => m,
            None => RelationshipMemory::new(target_id, target_name),
        };

        // 更新好感度
        if let Some(delta) = favorability_delta {
            memory.update_favorability(delta);
        }

        // 添加事件
        if let Some((event_type, description, favorability_delta, tick_id)) = event {
            memory.add_event(KeyEvent::new(
                tick_id,
                &event_type,
                &description,
                favorability_delta,
            ));
        }

        // 持久化
        self.store.upsert_relationship(&memory)?;

        Ok(memory)
    }
}
