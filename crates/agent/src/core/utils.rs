use crate::component::persona::LifespanCalculator;
use crate::models::WorldState;

/// 构建世界上下文
pub fn build_world_context(
    world_state: &WorldState,
    lifespan_calculator: Option<&LifespanCalculator>,
) -> String {
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

    if let Some(calculator) = lifespan_calculator {
        context.push_str(&format!(
            "\n年龄状态：{}",
            calculator.get_narrative_description()
        ));
    }

    context
}
