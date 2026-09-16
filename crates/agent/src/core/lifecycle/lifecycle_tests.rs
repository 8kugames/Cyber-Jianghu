//! lifecycle 模块单测（自 mod.rs 外移，内容未改）

use super::*;
use cyber_jianghu_protocol::WorldEventType;

fn event(tick: i64, desc: &str) -> WorldEvent {
    WorldEvent {
        event_type: WorldEventType::DeathNotification,
        tick_id: tick,
        description: desc.to_string(),
        metadata: serde_json::json!({}),
    }
}

#[test]
fn merge_events_log_appends_unseen_and_dedups() {
    let base = vec![event(1, "张三在 龙门大堂 亡故"), event(1, "有人说: 你好")];
    // 队列含一个重复事件（最新快照已携带）+ 一个被覆盖快照独有的事件
    let pending = vec![
        event(1, "有人说: 你好"),
        event(1, "你被 李四 攻击，损失 5 点气血"),
    ];

    let merged = merge_events_log(base, pending);

    assert_eq!(merged.len(), 3, "重复事件去重，独有事件保留");
    assert_eq!(merged[0].description, "张三在 龙门大堂 亡故");
    assert_eq!(merged[2].description, "你被 李四 攻击，损失 5 点气血");
}

#[test]
fn merge_events_log_empty_pending_is_noop() {
    let base = vec![event(1, "a")];
    let merged = merge_events_log(base, vec![]);
    assert_eq!(merged.len(), 1);
}

#[test]
fn merge_events_log_keeps_same_description_different_metadata() {
    // 同描述不同 metadata（如同名不同死者）不得误去重
    let e1 = WorldEvent {
        event_type: WorldEventType::DeathNotification,
        tick_id: 1,
        description: "有人亡故".to_string(),
        metadata: serde_json::json!({"agent_id": "a"}),
    };
    let e2 = WorldEvent {
        metadata: serde_json::json!({"agent_id": "b"}),
        ..e1.clone()
    };
    let merged = merge_events_log(vec![e1], vec![e2]);
    assert_eq!(merged.len(), 2, "metadata 不同即不同事件");
}
