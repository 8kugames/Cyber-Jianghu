// ============================================================================
// 遗忘调度器（艾宾浩斯曲线）
// ============================================================================
// 设计文档: (项目根)/docs/superpowers/specs/2025-03-15-semantic-memory-and-forgetting-design.md

use crate::component::memory::types::{EbbinghausConfig, MemoryEntry};
use chrono::Utc;

/// 遗忘调度器
pub struct ForgettingScheduler {
    config: EbbinghausConfig,
    last_forget_tick: i64,
    forget_interval: i64,
}

impl ForgettingScheduler {
    /// 创建新的遗忘调度器
    pub fn new(config: EbbinghausConfig, start_tick: i64, forget_interval: i64) -> Self {
        Self {
            config,
            last_forget_tick: start_tick,
            forget_interval,
        }
    }

    /// 使用默认配置创建
    pub fn with_default_config(start_tick: i64) -> Self {
        Self::new(EbbinghausConfig::default(), start_tick, 84)
    }

    /// 计算记忆保留率 R = e^(-t/S)
    pub fn calculate_retention(&self, memory: &MemoryEntry, current_tick: i64) -> f32 {
        let ticks_elapsed = current_tick.saturating_sub(memory.tick_id) as f32;
        let strength = memory.strength.max(0.01); // 避免除零
        (-self.config.decay_rate * ticks_elapsed / strength).exp()
    }

    /// 检索增强：每次被访问时增强记忆强度
    pub fn strengthen(&self, memory: &mut MemoryEntry) {
        memory.access_count += 1;
        memory.last_accessed_at = Some(Utc::now());
        // 强度增强：基于艾宾浩斯间隔重复效应
        memory.strength += self.config.retrieval_boost * (1.0 - memory.strength);
        memory.strength = memory.strength.min(1.0);
    }

    /// 是否应该执行遗忘
    pub fn should_run(&self, current_tick: i64) -> bool {
        current_tick - self.last_forget_tick >= self.forget_interval
    }

    /// 标记已执行
    pub fn mark_executed(&mut self, tick: i64) {
        self.last_forget_tick = tick;
    }

    /// 获取配置
    pub fn config(&self) -> &EbbinghausConfig {
        &self.config
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_retention() {
        let config = EbbinghausConfig::default();
        let scheduler = ForgettingScheduler::new(config, 0, 84);

        let memory = MemoryEntry {
            tick_id: 0,
            strength: 0.5,
            ..Default::default()
        };

        // 84 tick 后
        let retention = scheduler.calculate_retention(&memory, 84);
        assert!(retention > 0.0 && retention < 1.0);

        // 高强度记忆保留更久
        let strong_memory = MemoryEntry {
            tick_id: 0,
            strength: 0.9,
            ..Default::default()
        };
        let strong_retention = scheduler.calculate_retention(&strong_memory, 84);
        assert!(strong_retention > retention);
    }

    #[test]
    fn test_strengthen() {
        let config = EbbinghausConfig::default();
        let scheduler = ForgettingScheduler::new(config, 0, 84);

        let mut memory = MemoryEntry {
            tick_id: 0,
            strength: 0.5,
            access_count: 0,
            last_accessed_at: None,
            ..Default::default()
        };

        scheduler.strengthen(&mut memory);

        assert_eq!(memory.access_count, 1);
        assert!(memory.strength > 0.5);
        assert!(memory.last_accessed_at.is_some());
        assert!(memory.strength <= 1.0);
    }

    #[test]
    fn test_should_run() {
        let scheduler = ForgettingScheduler::with_default_config(0);

        assert!(!scheduler.should_run(83));
        assert!(scheduler.should_run(84));
        assert!(scheduler.should_run(168));
    }

    #[test]
    fn test_mark_executed() {
        let mut scheduler = ForgettingScheduler::with_default_config(0);

        assert!(scheduler.should_run(84));
        scheduler.mark_executed(84);
        assert!(!scheduler.should_run(167));
        assert!(scheduler.should_run(168));
    }
}
