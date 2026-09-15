// ============================================================================
// 三魂循环支撑：输出类型、LLM 失败判据、chaos 覆写留痕
// ============================================================================
//
// 自 soul_cycle.rs 拆出（.rs<800 行约束）：主循环文件只保留
// run_three_soul_cycle 本体，支撑类型与后置留痕方法归此文件。
// ============================================================================

use crate::models::Intent;

/// 意图是否为 LLM 失败产物（chaos 替补 / 认知失败兜底 / 配额耗尽）。
///
/// LLM 失败追踪与 chaos 恢复探测共用同一判据：chaos 模式下每 tick
/// 保留的人魂 LLM 输出若通过此判定（非失败产物），即视为 LLM 已恢复。
pub(super) fn intent_indicates_llm_failure(intent: &Intent) -> bool {
    intent.chaos_marker.is_some()
        || intent
            .thought_log
            .as_ref()
            .map(|t| {
                t.contains("意图多次被驳回")
                    || t.contains("三魂循环未产出有效意图")
                    || t.contains("认知失败")
                    || t.contains("[LLM 配额耗尽")
            })
            .unwrap_or(false)
}

/// 三魂循环输出
pub(crate) struct SoulCycleResult {
    pub intent: Intent,
    pub validated: bool,
    /// 最后一次尝试的序号（调用方做 chaos 覆写留痕时定位记录行）
    pub attempt: i32,
}

impl super::super::Agent {
    /// 人魂决策完成后的即时留痕：人魂输出 + 地魂 tool call + 世界时间
    /// （天魂审查前写入，model_id 取该次人魂实际使用的 LLM 模型，
    /// 含降级后真实模型，写入经历日志）
    pub(crate) async fn record_renhun_trace(
        &self,
        world_state: &cyber_jianghu_protocol::WorldState,
        attempt: i32,
        renhun_narrative: &str,
        renhun_thought_log: &str,
    ) {
        let attempt_model_id = self.actor_model_name().await;
        let Some(recorder) = self.soul_recorder().await else {
            return;
        };
        recorder
            .record_renhun(
                world_state.tick_id,
                attempt,
                renhun_narrative,
                renhun_thought_log,
                &attempt_model_id,
            )
            .await;
        if let Some(ref engine) = self.cognitive_engine
            && let Some(tool_calls) = engine.take_last_tool_call_log()
            && !tool_calls.is_empty()
            && let Ok(json) = serde_json::to_string(&tool_calls)
        {
            recorder
                .record_earth_tool_calls(world_state.tick_id, attempt, &json)
                .await;
        }
        let world_time_str = Self::format_world_time(&world_state.world_time);
        recorder
            .record_world_time(world_state.tick_id, attempt, &world_time_str)
            .await;
    }

    /// chaos 覆写留痕：后置替换发生时，既有记录要么已按原 pipeline 落库
    /// （认知失败休整替换）、要么根本没有 final intent 记录（fallback 被驳回
    /// 后的二次 chaos）。此处将实际发送的 chaos 意图覆写进 final intent 列，
    /// 并在天魂理由追加替换说明——保证面板「地魂」与实际执行一致、「天魂」
    /// 可追溯替换缘由。
    pub(crate) async fn record_chaos_override(
        &self,
        tick_id: i64,
        attempt: i32,
        intent: &Intent,
        note: &str,
    ) {
        let Some(recorder) = self.soul_recorder().await else {
            return;
        };
        let pipeline = Self::assemble_pipeline(vec![intent.clone()]);
        let action_data = pipeline
            .action_data
            .as_ref()
            .and_then(|d| serde_json::to_string(d).ok());
        let pipeline_actions = vec![serde_json::json!({
            "action_type": pipeline.action_type,
            "action_data": pipeline.action_data,
            "intent_id": pipeline.intent_id,
        })];
        let pipeline_json = serde_json::to_string(&pipeline_actions).ok();
        recorder
            .record_final_intent(
                tick_id,
                attempt,
                Some(&pipeline.intent_id.to_string()),
                Some(pipeline.action_type.as_str()),
                action_data.as_deref(),
                pipeline_json.as_deref(),
            )
            .await;
        recorder.append_tianhun_reason(tick_id, attempt, note).await;
    }
}

#[cfg(test)]
mod tests {
    use super::intent_indicates_llm_failure;
    use crate::models::Intent;
    use crate::soul::reflector::prompt::contains_meta_game_term;
    use uuid::Uuid;

    #[test]
    fn test_intent_indicates_llm_failure() {
        // 健康 LLM 输出：非失败产物（chaos 恢复探测据此判定可退出 chaos）
        let healthy = Intent::new(Uuid::nil(), 1, "移动", None);
        assert!(!intent_indicates_llm_failure(&healthy));

        // chaos 替补意图（带 chaos_marker）恒为失败产物
        let chaos = Intent::new(Uuid::nil(), 1, "取", None).with_chaos_marker(
            cyber_jianghu_protocol::types::ChaosMarker::LlmQuotaExhausted {
                consecutive_failures: 12,
            },
        );
        assert!(intent_indicates_llm_failure(&chaos));

        // 认知失败兜底文案各形态
        for thought in [
            "意图多次被驳回",
            "三魂循环未产出有效意图",
            "认知失败",
            "[LLM 配额耗尽] 已降级",
        ] {
            let mut fallback = Intent::new(Uuid::nil(), 1, "休整", None);
            fallback.thought_log = Some(thought.to_string());
            assert!(intent_indicates_llm_failure(&fallback), "漏判: {}", thought);
        }
    }

    #[test]
    fn test_meta_term_detection_matches_reflector_ban_list() {
        // 元术语必须拦截（"温九辞被记忆标成 NPC"回归）
        assert!(contains_meta_game_term("关键NPC交互：温九辞递来一壶酒"));
        assert!(contains_meta_game_term("我的HP只剩4点"));
        assert!(contains_meta_game_term("这个玩家很友善"));
        assert!(contains_meta_game_term("等级提升了，经验值大增"));

        // 正常武侠叙事不得误伤
        assert!(!contains_meta_game_term("与温九辞对饮，谈笑甚欢"));
        assert!(!contains_meta_game_term("客栈中小憩，恢复体力"));
        assert!(!contains_meta_game_term("act as 一名镖师押送货物"));
        // ASCII 词仅大写精确匹配：英文普通词含 mp/san 不误伤
        assert!(!contains_meta_game_term("扎营过夜 camp，sample 货物"));
        // ASCII 词为大小写敏感子串匹配：小写 hp/npc 是当前设计的绕过口，
        // 用测试固化该取舍（防 camp/sample 误伤优先）
        assert!(!contains_meta_game_term("hp 只剩4点"));
        assert!(!contains_meta_game_term("这个 npc 很友善"));
    }
}
