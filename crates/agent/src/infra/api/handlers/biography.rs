// 传记端点
// ============================================================================

use axum::{extract::State, response::Json};
use tracing::{error, info, warn};
use uuid::Uuid;

use super::HttpApiState;
use super::character_helpers::{get_character_by_id_sync, save_character};
use anyhow::Context;
use axum::http::StatusCode;
use axum::response::IntoResponse;

/// GET /api/v1/character/biography?agent_id=xxx
///
/// 返回已缓存的纪传体传记（不触发生成）
pub(crate) async fn get_biography_handler(
    State(state): State<HttpApiState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
    let agent_id = match resolve_biography_agent_id(&state, &params).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let character_dir = state.character_dir.read().await.clone();
    let character = match get_character_by_id_sync(&character_dir, agent_id) {
        Ok(Some(c)) => c,
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "character not found"})),
            )
                .into_response();
        }
    };

    match &character.biography {
        Some(bio) if !bio.is_empty() => Json(serde_json::json!({"biography": bio})).into_response(),
        _ => Json(serde_json::json!({"biography": null})).into_response(),
    }
}

/// POST /api/v1/character/biography?agent_id=xxx
///
/// 从三魂循环 + 每日摘要生成纪传体传记，写入 character.yaml
pub(crate) async fn generate_biography_handler(
    State(state): State<HttpApiState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
    let agent_id = match resolve_biography_agent_id(&state, &params).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    match generate_biography_for_agent(&state, agent_id).await {
        Ok(bio) => Json(serde_json::json!({"biography": bio})).into_response(),
        Err(e) => {
            let msg = e.to_string();
            let status = if msg.contains("not found") {
                StatusCode::NOT_FOUND
            } else if msg.contains("LLM") || msg.contains("无经历数据") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            error!("[biography] 生成失败: {}", msg);
            (status, Json(serde_json::json!({"error": msg}))).into_response()
        }
    }
}

/// 从 query params 解析 agent_id（优先取参数，否则取当前角色）
async fn resolve_biography_agent_id(
    state: &HttpApiState,
    params: &std::collections::HashMap<String, String>,
) -> Result<Uuid, axum::response::Response> {
    if let Some(id_str) = params.get("agent_id") {
        uuid::Uuid::parse_str(id_str).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Invalid agent_id"})),
            )
                .into_response()
        })
    } else {
        Ok(*state.agent_id.read().await)
    }
}

/// 收集三魂循环数据，格式化为可读时间线（倒序 → 正序输出）
async fn collect_soul_cycle_timeline(
    state: &HttpApiState,
    agent_id: Uuid,
) -> anyhow::Result<String> {
    let recorder = state
        .soul_recorder_for(agent_id)
        .await
        .context("soul cycle recorder not found")?;

    // 获取全部 tick_id 列表
    let (tick_ids, _total) = recorder.get_tick_ids_page(1, 1000).await;
    let all_records = recorder.get_by_ticks(&tick_ids).await;

    let mut lines: Vec<String> = Vec::new();
    // 按 tick_id 正序排列（原始是倒序）
    let mut sorted = tick_ids;
    sorted.sort();

    for tick_id in sorted {
        let records: Vec<_> = all_records
            .iter()
            .filter(|r| r.tick_id == tick_id)
            .collect();
        if records.is_empty() {
            continue;
        }

        let first = &records[0];
        let wt = first.world_time.as_deref().unwrap_or("-");
        lines.push(format!("\n--- Tick {} ({}) ---", tick_id, wt));

        for rec in &records {
            // 行动摘要：优先使用 pipeline 完整视图，覆盖所有 intent
            if let Some(ref action_type) = rec.final_action_type {
                let pipeline_items: Vec<serde_json::Value> = rec
                    .final_pipeline_json
                    .as_ref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_else(|| {
                        // 旧数据无 final_pipeline_json，退化为单 intent
                        let mut item = serde_json::json!({
                            "action_type": action_type,
                        });
                        if let Some(ref d) = rec.final_action_data
                            && let Ok(v) = serde_json::from_str::<serde_json::Value>(d)
                        {
                            item["action_data"] = v;
                        }
                        vec![item]
                    });

                let descs: Vec<String> = pipeline_items
                    .iter()
                    .map(|item| {
                        let at = item.get("action_type").and_then(|v| v.as_str()).unwrap_or("");
                        let content = item
                            .get("action_data")
                            .and_then(|d| d.get("content"))
                            .and_then(|c| c.as_str())
                            .unwrap_or("");
                        if (at == "speak" || at == "whisper" || at == "shout") && !content.is_empty()
                        {
                            format!("{}：{}", at, content)
                        } else {
                            at.to_string()
                        }
                    })
                    .collect();
                lines.push(format!("  行动：{}", descs.join(" → ")));
            }

            // 人魂叙事（简短摘要）
            if let Some(ref narrative) = rec.renhun_narrative {
                let truncated = narrative.clone();
                let ellipsis = "";
                lines.push(format!("  感知：{}{}", truncated, ellipsis));
            }
        }
    }

    Ok(lines.join("\n"))
}

/// 从 server Dashboard API 获取角色的每日摘要
async fn fetch_daily_summaries(state: &HttpApiState, agent_id: Uuid) -> anyhow::Result<String> {
    let server_http_url = state.server_http_url.read().await.clone();
    let url = format!(
        "{}/api/dashboard/agent-daily-summaries/{}?limit=100",
        server_http_url, agent_id
    );
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .context("请求每日摘要 API 超时或失败")?;

    if !resp.status().is_success() {
        anyhow::bail!("server 返回 {}", resp.status());
    }

    let body: serde_json::Value = resp.json().await.context("解析每日摘要响应失败")?;
    let summaries = body
        .get("summaries")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();

    if summaries.is_empty() {
        return Ok(String::new());
    }

    let mut lines: Vec<String> = Vec::new();
    for s in &summaries {
        let game_day = s.get("game_day").and_then(|d| d.as_i64()).unwrap_or(0);
        let summary = s.get("summary").and_then(|t| t.as_str()).unwrap_or("");
        if !summary.is_empty() {
            lines.push(format!("--- 第{}日 ---\n{}", game_day, summary));
        }
    }

    Ok(lines.join("\n\n"))
}

// ============================================================================

// 传记生成核心逻辑（HTTP handler + lifecycle 共用）
// ============================================================================

/// 为指定 agent 生成纪传体传记
///
/// 核心逻辑：收集三魂循环 + 每日摘要 → LLM 生成 → 保存 character.yaml → 回传 server。
/// HTTP handler 和 lifecycle 死亡回调共用此函数。
pub(crate) async fn generate_biography_for_agent(
    state: &HttpApiState,
    agent_id: Uuid,
) -> anyhow::Result<String> {
    use crate::component::llm::LlmClientExt;

    let character_dir = state.character_dir.read().await.clone();
    let mut character = match get_character_by_id_sync(&character_dir, agent_id)? {
        Some(c) => c,
        None => anyhow::bail!("character not found: {}", agent_id),
    };

    // 已有传记直接返回
    if let Some(ref bio) = character.biography
        && !bio.is_empty()
    {
        return Ok(bio.clone());
    }

    // 1. 收集三魂循环数据
    let timeline = collect_soul_cycle_timeline(state, agent_id).await?;

    // 2. 从 server 获取每日摘要
    let daily_summaries = match fetch_daily_summaries(state, agent_id).await {
        Ok(s) => s,
        Err(e) => {
            warn!("[biography] 获取每日摘要失败（非致命）: {}", e);
            String::new()
        }
    };

    // 3. 构建 prompt
    let char_info = format!(
        "姓名：{}\n年龄：{}\n性别：{}\n身份：{}\n性格：{}\n价值观：{}",
        character.name,
        character.age,
        character.gender,
        character.identity.as_deref().unwrap_or("未知"),
        character.personality.join("、"),
        character.values.join("、"),
    );

    // 构建经历日志段落（有数据则用，无数据则留空让 LLM 基于人物信息虚构）
    let timeline_section = if timeline.is_empty() {
        "（无经历日志）".to_string()
    } else {
        timeline
    };

    let daily_section = if daily_summaries.is_empty() {
        String::new()
    } else {
        format!("\n## 每日摘要（按游戏日排列）\n{}", daily_summaries)
    };

    let prompt = format!(
        r#"你是一位精通中国古典文学的传记作家。请根据以下角色的经历日志和每日摘要，以「纪传体」风格撰写一篇角色传记。

## 角色信息
{char_info}

## 经历日志（按时间顺序）
{timeline_section}
{daily_section}

## 撰写要求
1. 以第三人称叙述，开头简述角色籍贯出身（可合理虚构）
2. 按时间顺序叙述角色的关键经历：重要行动、人际交往、生死抉择
3. 语言风格：半文半白的武侠叙事，典雅凝练
4. 结尾以"论曰"或"太史公曰"附一段简短评语
5. 总字数：不少于100字，不超过2000字
6. 只输出传记正文，不要标题、不要标注字数、不要其他格式

请直接输出传记正文："#,
        char_info = char_info,
        timeline_section = timeline_section,
        daily_section = daily_section,
    );

    // 4. 获取共享 LLM 客户端
    let llm_client: std::sync::Arc<dyn crate::component::llm::LlmClient> = {
        let guard = state.llm_container.read().await;
        match guard.as_ref() {
            Some(container) => container.read().await.clone(),
            None => anyhow::bail!("LLM 未初始化，无法生成传记"),
        }
    };

    // 5. 调用 LLM
    #[derive(Debug, serde::Deserialize)]
    struct BiographyOutput {
        biography: String,
    }

    let json_prompt = format!(
        "{}\n\n请严格输出 JSON 格式：{{\"biography\": \"传记正文\"}}",
        prompt
    );

    let output = llm_client
        .complete_json::<BiographyOutput>(&json_prompt)
        .await?;
    let bio = output.biography.trim().to_string();

    // 来源：LLM prompt 要求"不少于100字不超过2000字"，10 为容低下限
    const BIOGRAPHY_MIN_CHARS: usize = 10;
    // 来源：LLM prompt 要求"不超过2000字"
    const BIOGRAPHY_MAX_CHARS: usize = 2000;

    if bio.chars().count() < BIOGRAPHY_MIN_CHARS {
        anyhow::bail!("生成的传记过短（少于{}字）", BIOGRAPHY_MIN_CHARS);
    }
    let bio: String = if bio.chars().count() > BIOGRAPHY_MAX_CHARS {
        info!(
            "[biography] 传记超长({}字)，截断至{}字",
            bio.chars().count(),
            BIOGRAPHY_MAX_CHARS
        );
        bio.chars().take(BIOGRAPHY_MAX_CHARS).collect()
    } else {
        bio
    };

    // 6. 存入 character.yaml
    character.biography = Some(bio.clone());
    if let Err(e) = save_character(&character, &character_dir) {
        error!("[biography] 保存传记失败: {}", e);
    }

    info!(
        "[biography] 传记生成成功: {} ({}字)",
        character.name,
        bio.chars().count()
    );

    // 7. 回传传记到 server（fire-and-forget）
    let server_http_url = state.server_http_url.read().await.clone();
    let bio_for_send = bio.clone();
    let agent_id_for_send = agent_id;
    tokio::spawn(async move {
        let url = format!("{}/api/v1/agent/biography", server_http_url);
        let client = reqwest::Client::new();
        let body = serde_json::json!({
            "agent_id": agent_id_for_send.to_string(),
            "biography": bio_for_send,
        });
        match client
            .post(&url)
            .json(&body)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                info!("[biography] 传记已回传 server");
            }
            Ok(resp) => {
                warn!("[biography] 传记回传 server 失败: status={}", resp.status());
            }
            Err(e) => {
                warn!("[biography] 传记回传 server 网络错误: {}", e);
            }
        }
    });

    Ok(bio)
}
