// ============================================================================
// 寿命计算器
// ============================================================================

use super::types::{AgingEffectValues, AgingStage, LifespanConfig, LifespanStatus};

/// 时间换算常量
///
/// 现实时间    Tick    游戏时间    年龄变化
/// ────────────────────────────────────────
/// 60秒        1       2小时       -
/// 72小时      4320    1年         +1岁
const TICKS_PER_GAME_YEAR: u64 = 4320; // 72 hours / 60 seconds = 4320 ticks

/// 寿命计算器
#[derive(Debug, Clone)]
pub struct LifespanCalculator {
    /// 配置
    config: LifespanConfig,

    /// 当前年龄
    current_age: u8,

    /// 已经过的 tick 数（用于计算年龄增长）
    ticks_elapsed: u64,
}

impl LifespanCalculator {
    /// 创建新的寿命计算器
    pub fn new(config: LifespanConfig) -> Self {
        let initial_age = config.initial_age;
        Self {
            config,
            current_age: initial_age,
            ticks_elapsed: 0,
        }
    }

    /// 使用默认配置创建
    pub fn with_default_config() -> Self {
        Self::new(LifespanConfig::default())
    }

    /// 设置初始年龄
    pub fn with_initial_age(mut self, age: u8) -> Self {
        self.current_age = age;
        self.config.initial_age = age;
        self
    }

    /// 获取当前年龄
    pub fn current_age(&self) -> u8 {
        self.current_age
    }

    /// 获取配置
    pub fn config(&self) -> &LifespanConfig {
        &self.config
    }

    /// 处理 tick 更新
    ///
    /// 返回更新后的寿命状态
    pub fn process_tick(&mut self) -> LifespanStatus {
        self.ticks_elapsed += 1;

        // 计算应该增加的年龄
        let game_years_passed = (self.ticks_elapsed as f32
            / TICKS_PER_GAME_YEAR as f32
            / self.config.aging_rate) as u64;

        // 计算新年龄
        let new_age = self
            .config
            .initial_age
            .saturating_add(game_years_passed as u8);

        // 更新年龄
        if new_age != self.current_age {
            self.current_age = new_age.min(self.config.max_age);
        }

        self.get_status()
    }

    /// 获取当前寿命状态
    pub fn get_status(&self) -> LifespanStatus {
        // 检查是否寿终
        if self.current_age >= self.config.max_age {
            return LifespanStatus::deceased(self.config.max_age);
        }

        // 检查是否老化
        if self.current_age >= self.config.aging_effects.aging_start_age {
            let effects = self.calculate_aging_effects();
            return LifespanStatus::aging(self.current_age, effects);
        }

        // 存活
        LifespanStatus::alive(self.current_age)
    }

    /// 计算老化效果
    fn calculate_aging_effects(&self) -> AgingEffectValues {
        let years_aging = self
            .current_age
            .saturating_sub(self.config.aging_effects.aging_start_age);

        let stamina_penalty = years_aging as f32 * self.config.aging_effects.stamina_decay;
        let hp_penalty = years_aging as f32 * self.config.aging_effects.hp_decay;

        let stage = self.determine_aging_stage();

        AgingEffectValues {
            stamina_penalty,
            hp_penalty,
            stage,
        }
    }

    /// 确定衰老阶段
    fn determine_aging_stage(&self) -> AgingStage {
        match self.current_age {
            0..=39 => AgingStage::Prime,
            40..=59 => AgingStage::MiddleAge,
            60..=74 => AgingStage::Old,
            _ => AgingStage::Venerable,
        }
    }

    /// 获取叙事描述
    ///
    /// 返回当前年龄的叙事描述，用于 LLM 上下文
    pub fn get_narrative_description(&self) -> String {
        let status = self.get_status();

        match status {
            LifespanStatus::Alive { age } => {
                format!("正值{}岁{}，身强体壮", age, self.get_age_narrative(age))
            }
            LifespanStatus::Aging { age, effects } => {
                format!(
                    "已{}岁{}，{}，体力略有衰退",
                    age,
                    self.get_age_narrative(age),
                    effects.stage
                )
            }
            LifespanStatus::Deceased { age } => {
                format!("寿终正寝，享年{}岁", age)
            }
        }
    }

    /// 获取年龄叙事
    fn get_age_narrative(&self, age: u8) -> &'static str {
        match age {
            0..=14 => "少年",
            15..=29 => "青年",
            30..=44 => "壮年",
            45..=59 => "中年",
            60..=74 => "花甲之年",
            _ => "耄耋之年",
        }
    }

    /// 估算剩余寿命（游戏年）
    pub fn estimated_remaining_years(&self) -> u8 {
        self.config.max_age.saturating_sub(self.current_age)
    }

    /// 估算剩余寿命（tick 数）
    pub fn estimated_remaining_ticks(&self) -> u64 {
        let remaining_years = self.estimated_remaining_years() as u64;
        remaining_years * TICKS_PER_GAME_YEAR * self.config.aging_rate as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lifespan_calculator_initial() {
        let calculator = LifespanCalculator::with_default_config();
        assert_eq!(calculator.current_age(), 28);

        let status = calculator.get_status();
        assert!(status.is_alive());
        assert_eq!(status.age(), 28);
    }

    #[test]
    fn test_lifespan_calculator_with_initial_age() {
        let calculator = LifespanCalculator::with_default_config().with_initial_age(45);

        assert_eq!(calculator.current_age(), 45);
    }

    #[test]
    fn test_lifespan_calculator_age_progression() {
        let mut calculator = LifespanCalculator::with_default_config().with_initial_age(28);

        // 模拟 4320 ticks（1 游戏年）
        for _ in 0..4320 {
            calculator.process_tick();
        }

        // 年龄应该增加 1
        assert_eq!(calculator.current_age(), 29);
    }

    #[test]
    fn test_lifespan_calculator_aging() {
        let calculator = LifespanCalculator::with_default_config().with_initial_age(60);

        let status = calculator.get_status();
        assert!(status.is_aging());
    }

    #[test]
    fn test_lifespan_calculator_deceased() {
        let calculator = LifespanCalculator::with_default_config().with_initial_age(80);

        let status = calculator.get_status();
        assert!(status.is_deceased());
    }

    #[test]
    fn test_narrative_description() {
        let calculator = LifespanCalculator::with_default_config().with_initial_age(25);
        let desc = calculator.get_narrative_description();
        assert!(desc.contains("25岁"));
        assert!(desc.contains("青年"));
    }

    #[test]
    fn test_estimated_remaining() {
        let calculator = LifespanCalculator::with_default_config().with_initial_age(28);

        assert_eq!(calculator.estimated_remaining_years(), 52);
    }
}
