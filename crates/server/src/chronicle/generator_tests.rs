//! generator 模块单测（自 generator.rs 外移，内容未改）

use super::*;
use crate::chronicle::ActionStats;
use crate::chronicle::collector::{AgentInfo, CollectedData};
use std::collections::HashMap;

#[test]
fn test_action_type_display() {
    crate::game_data::init_test_registry();
    assert_eq!(action_type_display("休整"), "静修");
    assert_eq!(action_type_display("说话"), "交谈");
    assert_eq!(action_type_display("移动"), "行走");
    assert_eq!(action_type_display("攻击"), "战斗");
    assert_eq!(action_type_display("unknown"), "unknown");
}

#[test]
fn test_template_generation() {
    crate::game_data::init_test_registry();
    let data = CollectedData {
        period_start: 1,
        period_end: 168,
        game_day_start: 1,
        game_day_end: 7,
        season: "春".to_string(),
        agents: vec![AgentInfo {
            agent_id: uuid::Uuid::new_v4(),
            name: "张三".to_string(),
            location: "village_center".to_string(),
            actions_count: 50,
            top_actions: vec![("移动".to_string(), 20), ("取".to_string(), 15)],
            narratives: vec!["在江湖中行走，感受春风".to_string()],
            died_this_period: false,
            retired_this_period: false,
        }],
        highlights: vec![],
        action_stats: ActionStats {
            total: 100,
            by_type: HashMap::from([
                ("移动".to_string(), 40),
                ("休整".to_string(), 30),
                ("取".to_string(), 30),
            ]),
            success_rate: 0.85,
        },
        location_stats: vec![],
        deaths: 2,
        births: 5,
        emergence_events: vec![],
        emergence_status: EmergenceCollectStatus::Ok,
    };

    let summary = generate_template(&data).unwrap();
    // game_day 1→一年元月一日, 7→一年元月七日（days_per_season=10, seasons_per_year=4）
    assert!(summary.contains("一年元月一日至一年元月七日"));
    assert!(summary.contains("春"));
    assert!(summary.contains("1 位江湖儿女"));
    assert!(summary.contains("100 次行动"));
}

/// AC: build_llm_prompt 含 agent narrative 文本（3a 信息源增强）
#[test]
fn test_llm_prompt_contains_narrative() {
    crate::game_data::init_test_registry();
    let data = CollectedData {
        period_start: 1,
        period_end: 168,
        game_day_start: 1,
        game_day_end: 7,
        season: "春".to_string(),
        agents: vec![AgentInfo {
            agent_id: uuid::Uuid::new_v4(),
            name: "李四".to_string(),
            location: "龙门客栈".to_string(),
            actions_count: 30,
            top_actions: vec![("说话".to_string(), 10)],
            narratives: vec!["今日与旧友重逢，感慨万千，决定共谋大事。".to_string()],
            died_this_period: false,
            retired_this_period: false,
        }],
        highlights: vec![],
        action_stats: ActionStats {
            total: 30,
            by_type: HashMap::new(),
            success_rate: 0.9,
        },
        location_stats: vec![],
        deaths: 0,
        births: 1,
        emergence_events: vec![],
        emergence_status: EmergenceCollectStatus::Ok,
    };

    let prompt = build_llm_prompt(&data, None);
    // narrative 应被注入 prompt（让 LLM 拿到角色素材）
    assert!(
        prompt.contains("今日与旧友重逢"),
        "LLM prompt 应包含 agent narrative"
    );
    assert!(prompt.contains("李四"));
}

/// AC: build_llm_prompt 含前情提要（跨周期人设一致性）
#[test]
fn test_llm_prompt_contains_previous_summary() {
    crate::game_data::init_test_registry();
    let data = CollectedData {
        period_start: 169,
        period_end: 336,
        game_day_start: 8,
        game_day_end: 14,
        season: "春".to_string(),
        agents: vec![],
        highlights: vec![],
        action_stats: ActionStats {
            total: 0,
            by_type: HashMap::new(),
            success_rate: 1.0,
        },
        location_stats: vec![],
        deaths: 0,
        births: 0,
        emergence_events: vec![],
        emergence_status: EmergenceCollectStatus::Ok,
    };

    let prev = "上一周期，张三与李四在龙门客栈结为生死之交。";
    let prompt = build_llm_prompt(&data, Some(prev));
    assert!(prompt.contains("前情提要"), "LLM prompt 应含前情提要段");
    assert!(
        prompt.contains("张三与李四在龙门客栈结为生死之交"),
        "前情提要应完整注入，不截断"
    );
}

/// AC: format_tick_range_chinese 把秒级 tick 转为中文日期（tick 不再当"日"渲染）
#[test]
fn test_format_tick_range_chinese() {
    crate::game_data::init_test_registry();
    // 测试配置: rspgd=60*1*24=1440；tick 2880→game_day 3, tick 4320→game_day 4
    // game_day 3→一年元月三日, 4→一年元月四日
    assert_eq!(
        format_tick_range_chinese(2880, 4320),
        "一年元月三日至一年元月四日"
    );
}

/// AC: build_llm_prompt 含涌现事件（3c 涌现流入 chronicle）
#[test]
fn test_llm_prompt_contains_emergence_events() {
    use crate::emergence::EmergenceEvent;
    crate::game_data::init_test_registry();
    let agent_a = uuid::Uuid::new_v4();
    let agent_b = uuid::Uuid::new_v4();
    let data = CollectedData {
        period_start: 1,
        period_end: 168,
        game_day_start: 1,
        game_day_end: 7,
        season: "春".to_string(),
        agents: vec![
            AgentInfo {
                agent_id: agent_a,
                name: "王五".to_string(),
                location: "龙门客栈".to_string(),
                actions_count: 20,
                top_actions: vec![],
                narratives: vec![],
                died_this_period: false,
                retired_this_period: false,
            },
            AgentInfo {
                agent_id: agent_b,
                name: "赵六".to_string(),
                location: "龙门客栈".to_string(),
                actions_count: 18,
                top_actions: vec![],
                narratives: vec![],
                died_this_period: false,
                retired_this_period: false,
            },
        ],
        highlights: vec![],
        action_stats: ActionStats {
            total: 38,
            by_type: HashMap::new(),
            success_rate: 0.9,
        },
        location_stats: vec![],
        deaths: 0,
        births: 2,
        emergence_events: vec![EmergenceEvent {
            category: "causal_emergence".to_string(),
            tick_start: 50,
            tick_end: 55,
            participants: vec![agent_a, agent_b],
            action_count: 5,
            categories_covered: vec!["conflict".to_string(), "trade".to_string()],
            causal_edges: vec![],
            actions: vec![],
        }],
        emergence_status: EmergenceCollectStatus::Ok,
    };

    let prompt = build_llm_prompt(&data, None);
    assert!(
        prompt.contains("因果涌现事件"),
        "LLM prompt 应包含涌现事件段"
    );
    assert!(prompt.contains("王五") && prompt.contains("赵六"));
}

/// 当期归隐/死亡角色区分标记：模板命运行与 LLM 人物简报各自呈现
#[test]
fn test_retire_and_death_marking() {
    crate::game_data::init_test_registry();
    let data = CollectedData {
        period_start: 1,
        period_end: 168,
        game_day_start: 1,
        game_day_end: 7,
        season: "春".to_string(),
        agents: vec![
            AgentInfo {
                agent_id: uuid::Uuid::new_v4(),
                name: "归隐客".to_string(),
                location: "龙门客栈".to_string(),
                actions_count: 10,
                top_actions: vec![],
                narratives: vec![],
                died_this_period: false,
                retired_this_period: true,
            },
            AgentInfo {
                agent_id: uuid::Uuid::new_v4(),
                name: "剑下亡魂".to_string(),
                location: "落霞镇".to_string(),
                actions_count: 8,
                top_actions: vec![],
                narratives: vec![],
                died_this_period: true,
                retired_this_period: false,
            },
        ],
        highlights: vec![],
        action_stats: ActionStats {
            total: 18,
            by_type: HashMap::new(),
            success_rate: 1.0,
        },
        location_stats: vec![],
        deaths: 1,
        births: 0,
        emergence_events: vec![],
        emergence_status: EmergenceCollectStatus::Ok,
    };

    let summary = generate_template(&data).unwrap();
    assert!(summary.contains("归隐于本周期"), "模板应标记归隐角色");
    assert!(summary.contains("陨落于本周期"), "模板应标记死亡角色");

    let prompt = build_llm_prompt(&data, None);
    assert!(prompt.contains("已归隐"), "LLM prompt 应标记归隐角色");
    assert!(prompt.contains("已陨落"), "LLM prompt 应标记死亡角色");
}
