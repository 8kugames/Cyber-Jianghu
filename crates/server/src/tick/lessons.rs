// ============================================================================
// 跨 Agent 传承 Layer 2: 共享教训库
// ============================================================================
//
// 死亡事件按 cause 聚合，达到阈值后自动生成/更新教训条目。
// 教训通过 WorldState.lessons_learned 下发给所有 Agent。
//
// 线程安全：使用 SQL INSERT ON CONFLICT DO UPDATE 原子操作，
// 不依赖调用方串行保证。

use std::collections::HashMap;

use sqlx::PgPool;
use tracing::{error, info};

use crate::game_data::types::unified_config::CauseAdvice;

/// 从 LessonConfig 的 cause_advice_map 构建 lesson 文本
///
/// 数据驱动：映射来自 game_rules.yaml 的 lesson.cause_advice_map 配置。
/// 未配置的 cause 使用通用 fallback 文本。
fn build_lesson_text(
    cause: &str,
    death_count: i32,
    avg_survival_ticks: i64,
    cause_map: &HashMap<String, CauseAdvice>,
) -> String {
    let (label, advice) = cause_map
        .get(cause)
        .map(|c| (c.label.as_str(), c.advice.as_str()))
        .unwrap_or(("未知原因", "请小心行事"));

    if avg_survival_ticks >= 0 {
        format!(
            "已有 {} 人因{}死亡，平均存活 {} tick。{}。",
            death_count, label, avg_survival_ticks, advice
        )
    } else {
        format!("已有 {} 人因{}死亡。{}。", death_count, label, advice)
    }
}

/// 记录一次死亡并原子更新教训库
///
/// 使用 INSERT ON CONFLICT DO UPDATE 保证原子性。
/// survival_ticks < 0（未知存活时间）时不纳入平均计算。
pub async fn record_death_lesson(
    db_pool: &PgPool,
    cause: &str,
    survival_ticks: i64,
    tick_id: i64,
    threshold: u32,
    cause_map: &HashMap<String, CauseAdvice>,
) {
    // survival_ticks < 0 表示 birth_tick 缺失，不纳入 avg 统计
    let valid_survival = if survival_ticks >= 0 {
        Some(survival_ticks)
    } else {
        None
    };

    // 原子 upsert + RETURNING：单次 SQL 拿回最新 count/avg
    let result = sqlx::query_as::<_, (i32, Option<i64>)>(
        r#"
        INSERT INTO public_lessons (cause, lesson, death_count, avg_survival_ticks, first_seen_tick, last_seen_tick)
        VALUES ($1, '', 1, $2, $3, $3)
        ON CONFLICT (cause) DO UPDATE SET
            death_count = public_lessons.death_count + 1,
            avg_survival_ticks = CASE
                WHEN $2 IS NOT NULL THEN
                    (COALESCE(public_lessons.avg_survival_ticks, 0) * public_lessons.death_count + $2)
                    / (public_lessons.death_count + 1)
                ELSE public_lessons.avg_survival_ticks
            END,
            last_seen_tick = $3,
            updated_at = NOW()
        RETURNING death_count, avg_survival_ticks
        "#,
    )
    .bind(cause)
    .bind(valid_survival)
    .bind(tick_id)
    .fetch_one(db_pool)
    .await;

    match result {
        Ok((count, avg_opt)) => {
            let avg = avg_opt.unwrap_or(-1);
            let lesson_text = build_lesson_text(cause, count, avg, cause_map);

            if let Err(e) = sqlx::query("UPDATE public_lessons SET lesson = $1 WHERE cause = $2")
                .bind(&lesson_text)
                .bind(cause)
                .execute(db_pool)
                .await
            {
                error!("[lesson] 更新教训文本失败: cause={}, error={}", cause, e);
            } else if count >= threshold as i32 {
                info!(
                    "[lesson] 教训已更新: cause={}, count={}, threshold={}",
                    cause, count, threshold
                );
            }
        }
        Err(e) => {
            error!("[lesson] 原子更新教训失败: cause={}, error={}", cause, e);
        }
    }
}

/// 查询达到阈值的教训（供 WorldState 下发）
///
/// 只返回 death_count >= threshold 的条目，按 death_count 降序排列。
pub async fn fetch_lessons_for_broadcast(
    db_pool: &PgPool,
    threshold: u32,
    limit: u32,
) -> Vec<cyber_jianghu_protocol::PublicLesson> {
    let rows = sqlx::query_as::<_, (String, String, i32, i64)>(
        "SELECT cause, lesson, death_count, COALESCE(avg_survival_ticks, 0) FROM public_lessons WHERE death_count >= $1 ORDER BY death_count DESC LIMIT $2",
    )
    .bind(threshold as i32)
    .bind(limit as i64)
    .fetch_all(db_pool)
    .await;

    match rows {
        Ok(rows) => rows
            .into_iter()
            .map(|(cause, lesson, death_count, avg_survival_ticks)| {
                cyber_jianghu_protocol::PublicLesson {
                    cause,
                    lesson,
                    death_count,
                    avg_survival_ticks,
                }
            })
            .collect(),
        Err(e) => {
            error!("[lesson] 查询教训列表失败: {}", e);
            vec![]
        }
    }
}
