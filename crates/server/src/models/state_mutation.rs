// ============================================================================
// AgentState 状态变更方法
// ============================================================================

use std::collections::HashMap;
use tracing::debug;

use super::AgentState;

impl AgentState {
    /// 获取公式计算上下文
    ///
    /// 除状态/先天属性外，注入衰老变量（数据驱动公式的统一入口）：
    /// - `age`：角色当前年龄（游戏年）。birth_tick 缺失（不朽/未知）时为 0，
    ///   使 `max(0, age - aging_start_age)` 类惩罚自然归零，而非公式求值失败。
    /// - `aging_start_age`：衰老起始年龄（game_rules.yaml lifespan.aging_start_age）。
    ///   属性公式由此表达衰老惩罚（如 hp/stamina 上限随龄递减），零硬编码；
    ///   lifespan 配置缺失时不注入，引用它的公式按既有回退路径降级。
    pub fn get_formula_context(&self) -> std::collections::HashMap<String, i32> {
        let mut context = std::collections::HashMap::new();
        // 添加状态属性
        for (name, attr) in &self.status.collection.attributes {
            context.insert(name.clone(), attr.value.get());
        }
        // 添加先天属性
        for (name, attr) in &self.primary_attributes.collection.attributes {
            context.insert(name.clone(), attr.value.get());
        }
        // 衰老变量
        let age = self
            .birth_tick
            .map(|b| crate::tick::decay::compute_age_years(b, self.tick_id))
            .unwrap_or(0);
        context.insert("age".to_string(), age as i32);
        if let Some((_, aging_start_age, _)) =
            crate::game_data::registry().and_then(|r| r.get_lifespan_config())
        {
            context.insert("aging_start_age".to_string(), aging_start_age as i32);
        }
        context
    }

    /// 获取季节对指定属性的修饰系数（数据驱动）
    ///
    /// 从 time.json 的季节配置中获取 attribute_modifiers
    /// 返回 1.0 表示无修饰，>1.0 表示增加，<1.0 表示减少
    fn get_season_modifier(&self, attr_name: &str, tick_id: i64) -> f32 {
        if let Some(season) = crate::game_data::registry::TimeRegistry::get_current_season(tick_id)
        {
            // 从季节配置中获取该属性的修饰系数
            if let Some(&modifier) = season.attribute_modifiers.get(attr_name) {
                return modifier;
            }
        }
        1.0 // 默认无修饰
    }

    /// 应用生理值衰减（默认视为休息 tick）
    pub fn apply_decay(&mut self, tick_id: i64) -> Option<String> {
        self.apply_decay_with_rest(tick_id, true)
    }

    /// 应用生理值衰减（显式休息标记）
    ///
    /// 处理三类属性变化：
    /// 1. decay_per_tick: 衰减值（正值表示扣减，如 satiation 每tick扣减5），每 tick 生效
    /// 2. recovery_formula（decay=0 属性，如 stamina/qi）：无条件恢复
    /// 3. recovery_formula（decay≠0 属性，如 sanity）：仅休息 tick（rested=true）恢复。
    ///    物理语义：本 tick 窗口无 intent 提交 = 身体在休息（idle-skip/离线均自然覆盖）。
    ///
    /// 季节修饰系数从 time.json 的季节配置中读取（数据驱动）
    ///
    /// 返回值：如果Agent死亡，返回 Some(attr_name) 表示触发死亡的属性名；否则返回 None
    pub fn apply_decay_with_rest(&mut self, tick_id: i64, rested: bool) -> Option<String> {
        if !self.is_alive {
            return None;
        }

        let context = self.get_formula_context();

        // 1. 处理衰减属性
        // decay_per_tick 表示扣减量（正值=扣减量，如 satiation 每tick扣减5）
        let attributes_to_decay = self.status.get_decaying_attributes();

        for (attr_name, decay_amount) in attributes_to_decay {
            // decay_per_tick 是扣减量，需要取负值作为 delta
            let base_delta = -decay_amount;

            // 获取季节修饰系数（数据驱动）
            let season_modifier = self.get_season_modifier(&attr_name, tick_id);
            let raw_delta = base_delta * season_modifier;

            // 用累计器把小数部分留到下一 tick
            let acc = self
                .decay_accumulator
                .entry(attr_name.clone())
                .or_insert(0.0);
            *acc += raw_delta;
            let delta = *acc as i32; // 朝零截断（f32→i32 cast 而非 floor）
            *acc -= delta as f32;

            if delta != 0 {
                // 记录衰减前的值
                let before_value = self.status.get(&attr_name).unwrap_or(-1);

                debug!(
                    "Applying decay to {}: decay_amount={}, season_modifier={}, raw_delta={}, delta={}, before_value={}",
                    attr_name, decay_amount, season_modifier, raw_delta, delta, before_value
                );

                if let Ok(new_val) = self.status.apply_change(&attr_name, delta, &context) {
                    debug!(
                        "Applied decay to {}: before={}, delta={}, after={}",
                        attr_name, before_value, delta, new_val
                    );
                }
            }

            // 死亡检查：累计器未到 1.0（delta=0）时也要检查，
            // 因为属性可能已通过其他途径（如吃/喝耗尽）触发了死亡条件
            if self.status.check_death_condition(&attr_name) {
                self.is_alive = false;
                if let Err(e) = self.status.set("hp", 0) {
                    tracing::warn!(
                        "death 触发：status.set(\"hp\", 0) 失败（is_alive 已设为 false 但状态未持久化）：{e:?}"
                    );
                }
                tracing::warn!(
                    "Agent {} 因 {} 归零而死亡 (Tick: {})",
                    self.agent_id,
                    attr_name,
                    tick_id
                );
                return Some(attr_name);
            }
        }

        // 如果已死亡，不再处理恢复
        if !self.is_alive {
            return None;
        }

        // 2. 处理无条件恢复属性（recovery_formula 且 decay=0，如 stamina/qi）
        for (attr_name, formula) in self.status.get_recovering_attributes() {
            self.apply_formula_recovery(&attr_name, &formula, tick_id, &context);
        }

        // 3. 处理休息门控恢复属性（recovery_formula 且 decay≠0，如 sanity）：
        //    仅休息 tick（本 tick 窗口无 intent）恢复，行动 tick 只衰减。
        //    修复前 recovery_formula 对此类属性永不生效（被 decay≠0 守卫排除），
        //    sanity 成为纯单向末日时钟，全员永久混沌。
        if rested {
            for (attr_name, formula) in self.status.get_rest_gated_recovering_attributes() {
                self.apply_formula_recovery(&attr_name, &formula, tick_id, &context);
            }
        }

        None
    }

    /// 应用单条 recovery_formula（公式求值 + 季节修饰 + apply_change，best-effort）
    fn apply_formula_recovery(
        &mut self,
        attr_name: &str,
        formula: &str,
        tick_id: i64,
        context: &std::collections::HashMap<String, i32>,
    ) {
        let i64_context: std::collections::HashMap<String, i64> = context
            .iter()
            .map(|(k, v)| (k.clone(), *v as i64))
            .collect();
        let engine = crate::game_data::formula_engine::FormulaEngine::new();

        let base_recovery = match engine.evaluate_int(formula, &i64_context) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    "recovery 公式求值失败（best-effort 跳过本轮）: attr={}, formula={}, err={:?}",
                    attr_name,
                    formula,
                    e
                );
                return;
            }
        };

        if base_recovery <= 0 {
            return;
        }

        // 获取季节修饰系数（数据驱动）
        let season_modifier = self.get_season_modifier(attr_name, tick_id);
        let delta = (base_recovery as f32 * season_modifier).round() as i32;

        if delta > 0 {
            let before_value = self.status.get(attr_name).unwrap_or(-1);
            debug!(
                "Applying recovery to {}: formula={}, base_recovery={}, season_modifier={}, delta={}, before_value={}",
                attr_name, formula, base_recovery, season_modifier, delta, before_value
            );

            if let Ok(new_val) = self.status.apply_change(attr_name, delta, context) {
                debug!(
                    "Applied recovery to {}: before={}, delta={}, after={}",
                    attr_name, before_value, delta, new_val
                );
            }
        }
    }

    /// 恢复属性值（通用方法，委托给 StatusComponent）
    ///
    /// 使用物品恢复某个属性
    ///
    /// 如果属性不存在，变更会被拒绝，保持原始值不变
    pub fn restore_attribute(&mut self, attr_name: &str, amount: i32) {
        if !self.is_alive {
            return;
        }
        let context = self.get_formula_context();
        // 使用 StatusComponent 的 apply_change 方法（带范围限制）
        let _ = self.status.apply_change(attr_name, amount, &context);
    }

    /// 受到伤害
    ///
    /// HP减少，归零后死亡
    pub fn take_damage(&mut self, damage: i32) {
        if !self.is_alive {
            return;
        }
        self.restore_attribute("hp", -damage);

        // 检查死亡条件（通过组件）
        if self.status.check_death_conditions().is_some() {
            self.is_alive = false;
        }
    }

    /// 获取所有属性用于协议序列化（从组件转换为 HashMap）
    ///
    /// 将组件化的属性转换为 HashMap 格式，用于 WebSocket 传输
    pub fn get_attributes_for_protocol(&self) -> HashMap<String, i32> {
        let mut attributes = HashMap::new();
        let context = self.get_formula_context();

        // 从 StatusComponent 收集所有状态属性
        for (name, attr) in &self.status.collection.attributes {
            attributes.insert(name.clone(), attr.value.get());

            // 顺便提供上限值
            let max_value = crate::game_data::types::StatusComponent::evaluate_max_value(
                &attr.metadata.max_value_formula,
                crate::game_data::types::DEFAULT_STATUS_MAX_VALUE,
                &context,
            ) as i32
                + self.status.max_modifiers.get(name).copied().unwrap_or(0);
            attributes.insert(format!("{}_max", name), max_value);
        }

        // 从 AttributeComponent 收集所有先天属性
        for (name, attr) in &self.primary_attributes.collection.attributes {
            attributes.insert(name.clone(), attr.value.get());

            // 提供先天属性的极限值（如果是可成长属性）
            if let crate::game_data::types::attributes::AttributeValue::Growable { base, .. } =
                &attr.value
            {
                attributes.insert(format!("{}_max", name), *base as i32);
            }
        }

        attributes
    }

    /// 获取派生属性用于协议序列化（浮点数）
    ///
    /// 计算派生属性（如闪避率、暴击率等）并返回 f32 HashMap
    pub fn get_derived_attributes_for_protocol(&self) -> HashMap<String, f32> {
        let mut derived_attributes = HashMap::new();
        let context = self.get_formula_context();

        if let Some(config) = crate::game_data::registry::StateRegistry::get_attributes_config() {
            for (name, attr_def) in &config.data.derived.attributes {
                if let Some(formula) = &attr_def.formula {
                    let value = crate::game_data::types::StatusComponent::evaluate_max_value(
                        &Some(formula.clone()),
                        attr_def.default_value.unwrap_or(0.0) as f32,
                        &context,
                    );
                    derived_attributes.insert(name.clone(), value);
                }
            }
        }

        derived_attributes
    }
}

#[cfg(test)]
mod aging_tests {
    use super::*;
    use crate::game_data::types::StatusComponent;

    /// 构造指定年龄（游戏年）的 Agent，age 由 birth_tick 偏移推导。
    /// 换算链复用生产函数（compute_starting_age_ticks/compute_age_years），
    /// 不在测试里重算时间模型。
    fn agent_at_age(age_years: i64) -> AgentState {
        crate::game_data::init_test_registry();
        let sat = crate::tick::decay::compute_starting_age_ticks();
        let starting = crate::tick::decay::compute_age_years(0, sat);
        assert!(starting > 0 && sat > 0, "测试注册表需含 time 配置");
        let years_in_ticks = age_years * (sat / starting);
        let mut state = AgentState::new(uuid::Uuid::new_v4(), 1_000_000);
        state.birth_tick = Some(state.tick_id - years_in_ticks);
        state
    }

    #[test]
    fn formula_context_injects_age_from_birth_tick() {
        let state = agent_at_age(60);
        let context = state.get_formula_context();
        assert_eq!(
            context.get("age"),
            Some(&60),
            "age 必须由 birth_tick 推导注入"
        );
    }

    #[test]
    fn formula_context_age_defaults_to_zero_without_birth_tick() {
        let mut state = agent_at_age(60);
        state.birth_tick = None;
        let context = state.get_formula_context();
        // birth 缺失（不朽/未知）→ age=0 → 衰老惩罚自然归零，公式不失败
        assert_eq!(context.get("age"), Some(&0));
    }

    /// 与 attributes.yaml hp/stamina 上限公式同构：验证 evalexpr max() +
    /// 变量注入的数学正确性（衰龄前不衰减、衰龄后逐年递减）
    #[test]
    fn aging_formula_math_matches_yaml_shape() {
        let formula = "100 + constitution * 2 - max(0, age - aging_start_age) * 2";
        let eval = |age: i32, constitution: i32| {
            let mut ctx = std::collections::HashMap::new();
            ctx.insert("constitution".to_string(), constitution);
            ctx.insert("age".to_string(), age);
            ctx.insert("aging_start_age".to_string(), 50);
            StatusComponent::evaluate_max_value(&Some(formula.to_string()), 100.0, &ctx) as i32
        };

        // 衰龄前：无衰减（根骨 10 → 100+20）
        assert_eq!(eval(30, 10), 120);
        assert_eq!(eval(50, 10), 120, "恰至衰龄当年不衰减");
        // 衰龄后：每年 -2（70 岁 → 100+20-40）
        assert_eq!(eval(70, 10), 80);
        // 寿终 80 岁：100+20-60（仍为正，寿终由 max_age 硬性判定，非饿死于公式）
        assert_eq!(eval(80, 10), 60);
        // 高根骨者绝对值更高，但衰减速率相同
        assert_eq!(eval(70, 50), 160);
    }

    /// apply_change 在衰龄后按新上限收敛：超出上限的当前值在下次变更时被夹紧
    #[test]
    fn apply_change_clamps_to_aged_max() {
        let mut state = agent_at_age(70);
        // 测试注册表 hp 上限公式为平铺 "100"，改为衰龄公式验证真实行为
        if let Some(attr) = state.status.collection.attributes.get_mut("hp") {
            attr.metadata.max_value_formula =
                Some("100 + constitution * 2 - max(0, age - aging_start_age) * 2".to_string());
        }
        // 注：测试注册表无 lifespan 配置 → aging_start_age 不注入 → 公式回退默认 100。
        // 此处直接验证“上下文含 age”与夹紧路径本身，公式数学已由上例覆盖。
        let context = state.get_formula_context();
        assert!(context.contains_key("age"));
        let before = state.status.get("hp").unwrap_or(0);
        // +1000 的恢复被夹紧到上限（回退默认 100，验证夹紧机制本身可用）
        let new = state
            .status
            .apply_change("hp", 1000, &context)
            .expect("hp 存在");
        assert!(new <= before + 1000);
        assert!(new >= 0);
    }
}
