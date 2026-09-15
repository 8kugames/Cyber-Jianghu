//! crates/server/tests/survival_pacing_test.rs
//!
//! 生存节奏锁定测试：对生产 config/attributes.yaml + time.yaml 断言
//! 「满值饿死/渴死 = 7 个游戏日」这一截断敏感性质。
//!
//! 衰减经衰减小数累计器（state_mutation.rs：acc += raw_delta;
//! delta = acc as i32 朝零截断; acc -= delta）逐 tick 扣减——该性质
//! 对取值敏感：decay=0.5955 时 167 tick 累计不足 100，死亡落在
//! 第 168 tick（第 7 游戏日末）；0.5955 的截断补偿恰对齐
//! 100/(7*24) = 0.5952 的理论均值。未来再调生存数值时，本测试
//! 保证目标节奏不被无意破坏。
//!
//! 运行: cargo test --test survival_pacing_test

#[cfg(test)]
mod tests {
    use cyber_jianghu_server::game_data::load_from_dir;

    /// 复刻 state_mutation.rs 的衰减小数累计器（季节系数固定 1.0 基准），
    /// 返回从满值起步到触发 equals-0 死亡所需的 tick 数。
    fn ticks_until_starvation(default_value: i64, min_value: i64, decay: f64) -> i64 {
        let mut value: i64 = default_value;
        let mut acc: f32 = 0.0;
        let mut tick: i64 = 0;
        loop {
            tick += 1;
            assert!(tick < 10_000, "10 万 tick 内未死亡，衰减配置疑似归零");
            // 与生产同序：acc 累积负向 delta，朝零截断取整扣减
            acc += (-decay) as f32;
            let delta = acc as i32;
            acc -= delta as f32;
            value = (value + delta as i64).max(min_value);
            if value <= min_value {
                return tick;
            }
        }
    }

    #[test]
    fn test_full_value_starvation_is_seven_game_days() {
        let data =
            load_from_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/config")).expect("加载生产配置");

        let time = &data.time.data;
        let ticks_per_day = (time.ticks_per_hour * time.hours_per_day) as i64;
        assert_eq!(
            ticks_per_day, 24,
            "time.yaml 游戏日长度变化时需重审生存节奏"
        );

        for attr in ["satiation", "hydration"] {
            let def = data
                .attributes
                .data
                .status
                .attributes
                .get(attr)
                .unwrap_or_else(|| panic!("attributes.yaml 缺少 {}", attr));
            let default_value = def.default_value.expect("default_value 必配") as i64;
            let min_value = def.min_value.unwrap_or(0.0) as i64;
            let decay = def.decay_per_tick.expect("decay_per_tick 必配");
            assert!(decay > 0.0, "{} 衰减非正值，生存压力机制失效", attr);

            let death_tick = ticks_until_starvation(default_value, min_value, decay);
            let target_days = 7;
            assert_eq!(
                death_tick,
                target_days * ticks_per_day,
                "{} 满值 {} 于第 {} tick 死亡，偏离 {} 游戏日目标（当前 decay={}）；\
                 调整 decay 时注意 100/(7*{}) = {:.4} 的截断补偿方向",
                attr,
                default_value,
                death_tick,
                target_days,
                decay,
                ticks_per_day,
                default_value as f64 / (target_days * ticks_per_day) as f64
            );
        }
    }
}
