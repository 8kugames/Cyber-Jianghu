use sqlx::Row;

/// 行级成败判定：只有 result 恰好是 'success' 才算成功
///
/// NULL 判为失败，与 `EXPERIENCE_ROW_FILTERS` 里 `result IS DISTINCT FROM
/// 'success'` 的语义一致（`IS DISTINCT FROM` 把 NULL 视为非成功）。判定标准
/// 全文件只此一处，供卡片级聚合与逐动作执行结果共用。
pub(super) fn row_success(result: Option<&str>) -> bool {
    result == Some("success")
}

/// 卡片级成败：一个 tick 的全部 pipe_seq 行都 success 才算成功
///
/// 与 `EXPERIENCE_ROW_FILTERS` 中 `$6 = 'success'` 的 `NOT EXISTS (... result IS
/// DISTINCT FROM 'success')` 是同一判定的两种写法（SQL 侧判入选、Rust 侧判徽章），
/// 因此本函数是那条 SQL 的孪生判定：任一改动必须同步另一处，`row_result_semantics`
/// 单测锁住两侧共同依赖的 NULL/值域语义，experience_stream_sql_test.rs 锁住
/// SQL 侧的划分性质。取数时该 tick 的全部行都在 `group` 内（取键按卡片分页、
/// 取数按 unnest 键取全量 pipe_seq），故判定完整。
///
/// 注意这是同一快照下的结论：计数/取键与取数分属两条自动提交语句，若其间该
/// tick 的行被写入改动（state_ops 的 UPSERT 可翻转 result、也可为既有 tick 补插
/// 行），卡片徽章与筛选入选可能短暂分叉——窗口为单次请求，非持久不一致。
pub(super) fn card_success(group: &[sqlx::postgres::PgRow]) -> bool {
    group
        .iter()
        .all(|row| row_success(row.get::<Option<String>, _>("result").as_deref()))
}

/// 按 pipe_seq 汇总一个 tick 内各条动作的 Server 执行结果
///
/// key = pipe_seq，value = {success, error, state_change_summary}。
/// 注入到 soul_cycle_metadata 后，前端按 pipe_seq 直接取用，
/// 无需再从扁平行数组里自行配对。
pub(super) fn build_execution_results(group: &[sqlx::postgres::PgRow]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for row in group {
        let pipe_seq: i32 = row.get("pipe_seq");
        let result: Option<String> = row.get("result");
        let result_msg: Option<String> = row.get("result_message");
        let is_success = row_success(result.as_deref());
        map.insert(
            pipe_seq.to_string(),
            serde_json::json!({
                "success": is_success,
                "error": if is_success { serde_json::Value::Null } else { serde_json::Value::String(result_msg.clone().unwrap_or_default()) },
                "state_change_summary": if is_success { serde_json::Value::String(result_msg.clone().unwrap_or_default()) } else { serde_json::Value::Null },
            }),
        );
    }
    serde_json::Value::Object(map)
}

/// 把执行结果注入 soul_cycle_metadata（metadata 为空时不构造）
pub(super) fn inject_execution_results(
    metadata: Option<serde_json::Value>,
    execution_results: &serde_json::Value,
) -> Option<serde_json::Value> {
    metadata.map(|mut m| {
        if let serde_json::Value::Object(ref mut obj) = m {
            obj.insert("execution_results".to_string(), execution_results.clone());
        }
        m
    })
}

/// 经历行模型 ID 的权威归一
///
/// 优先级：该 tick 内首个非空 per-attempt 模型（cycles[0] 链上的 model_id）
/// → 该 agent 注册时上报的 agents.model_id。
///
/// 早期实现只读主行的 cycles[0]，导致同一 tick 的后续意图行（simplified
/// metadata 不含 model_id）在经历日志里没有模型，而同一卡片的主行有模型。
/// 历史行两个来源皆空时返回 None，前端渲染为「模型未上报」，属不可回填的
/// 早期数据局限（字段当时尚未落库）。
///
/// 注意本函数不做空串 / "unknown" 归一：那是 agent 侧写入边界
/// （`normalize_model_id`）的职责，此处只按来源优先级取值。服务端现有数据
/// 中不存在这两种字面量（只读统计均为 0 行），故读路径不再重复判定。
pub(super) fn resolve_experience_model(
    group: &[sqlx::postgres::PgRow],
    agent_model_id: Option<String>,
) -> Option<String> {
    group
        .iter()
        .find_map(|row| row.get::<Option<String>, _>("per_row_model_id"))
        .or(agent_model_id)
}

#[cfg(test)]
mod tests {
    use super::row_success;

    /// 锁住卡片级判定与 SQL 筛选共同依赖的行级语义
    ///
    /// `EXPERIENCE_ROW_FILTERS` 用 `result IS DISTINCT FROM 'success'` 判定失败，
    /// 该算子把 NULL 视为"与 'success' 不同"，即 NULL 行算失败行。Rust 侧
    /// `row_success` 必须同样把 NULL 判为失败，否则同一 tick 的筛选入选与卡片徽章
    /// 会分叉。此处三条断言即该语义的可执行版本（SQL 侧的划分性质由
    /// tests/experience_stream_sql_test.rs 在活库上验证）。
    #[test]
    fn row_result_semantics_match_sql_predicate() {
        assert!(row_success(Some("success")));
        assert!(!row_success(Some("failed")));
        // NULL 与 SQL 的 `IS DISTINCT FROM 'success'` 同判为非成功
        assert!(!row_success(None));
    }
}
