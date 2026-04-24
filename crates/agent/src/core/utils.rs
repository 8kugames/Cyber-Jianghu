use crate::models::WorldState;

/// 构建世界上下文
///
/// 年龄信息从 Server 下发的 WorldState.self_state 读取（Server 权威）
pub fn build_world_context(world_state: &WorldState) -> String {
    let mut context = format!(
        "当前位置：{}\n时间：{}\n天气：{}",
        world_state.location.name,
        world_state.world_time.to_chinese(),
        world_state.world_time.weather,
    );

    if !world_state.entities.is_empty() {
        context.push_str("\n周围人物：");
        for entity in &world_state.entities {
            context.push_str(&format!("\n- {} ({})", entity.name, entity.state));
        }
    }

    // 年龄信息由 Server 计算（从 birth_tick + time.yaml 派生）
    if let Some(age) = world_state.self_state.age_years {
        let max = world_state.self_state.max_age.unwrap_or(80);
        context.push_str(&format!("\n年龄：{}岁（寿命上限{}岁）", age, max));
    }

    context
}
