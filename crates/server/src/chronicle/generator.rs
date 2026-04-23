// ============================================================================
// 生成器：模板 + LLM
// ============================================================================
//
// 模板生成：纯规则，无外部依赖，快速可靠
// LLM 生成：调用外部 LLM，增强叙事质量
// ============================================================================

use anyhow::{Context, Result};
use std::sync::atomic::{AtomicU64, Ordering};

use super::collector::CollectedData;

/// LLM Token 统计（全局）
static LLM_INPUT_TOKENS: AtomicU64 = AtomicU64::new(0);
static LLM_OUTPUT_TOKENS: AtomicU64 = AtomicU64::new(0);
static LLM_REQUEST_COUNT: AtomicU64 = AtomicU64::new(0);
static LLM_ERROR_COUNT: AtomicU64 = AtomicU64::new(0);

/// 获取 Token 统计
pub fn get_llm_stats() -> (u64, u64, u64, u64) {
    (
        LLM_INPUT_TOKENS.load(Ordering::Relaxed),
        LLM_OUTPUT_TOKENS.load(Ordering::Relaxed),
        LLM_REQUEST_COUNT.load(Ordering::Relaxed),
        LLM_ERROR_COUNT.load(Ordering::Relaxed),
    )
}

/// 记录 Token 使用
pub fn record_llm_tokens(input_tokens: u64, output_tokens: u64) {
    LLM_INPUT_TOKENS.fetch_add(input_tokens, Ordering::Relaxed);
    LLM_OUTPUT_TOKENS.fetch_add(output_tokens, Ordering::Relaxed);
    LLM_REQUEST_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// 记录 LLM 错误
pub fn record_llm_error() {
    LLM_ERROR_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// 动作类型中文映射（action_type 已是中文，此处仅做显示名美化）
fn action_type_display(action_type: &str) -> String {
    match action_type {
        "休息" => "静修".to_string(),
        "说话" => "交谈".to_string(),
        "移动" => "行走".to_string(),
        "攻击" => "战斗".to_string(),
        "给予" => "给予".to_string(),
        "偷窃" => "偷窃".to_string(),
        "制造" => "锻造".to_string(),
        "大喊" => "呼喊".to_string(),
        _ => action_type.to_string(),
    }
}

/// 事件类型中文映射
fn event_type_display(event_type: &str) -> String {
    match event_type {
        "death" => "陨落".to_string(),
        "dialogue" => "对话".to_string(),
        "combat" => "战斗".to_string(),
        "social" => "交际".to_string(),
        _ => event_type.to_string(),
    }
}

/// 模板生成
pub fn generate_template(data: &CollectedData) -> Result<String> {
    let mut summary = String::new();
    let nl = "\n"; // 统一换行符

    // 标题
    summary.push_str(&format!(
        "【第{}日至第{}日】{}季·群像传记{nl}{nl}",
        data.game_day_start, data.game_day_end, data.season
    ));

    // 概述
    let agent_count = data.agents.len() as i32;
    summary.push_str(&format!("## 江湖概述{nl}{nl}"));
    summary.push_str(&format!(
        "本周期共有 {} 位江湖儿女闯荡江湖，{nl}",
        agent_count
    ));
    summary.push_str(&format!(
        "共发生 {} 次行动记录，{nl}",
        data.action_stats.total
    ));

    let survival_rate = if agent_count > 0 {
        let survivors = agent_count - data.deaths;
        (survivors as f64 / agent_count as f64 * 100.0) as i32
    } else {
        100
    };
    summary.push_str(&format!(
        "其中 {} 人在激烈的生存竞争中陨落，存活率约 {}%。{nl}{nl}",
        data.deaths, survival_rate
    ));

    // 行动分布
    if !data.action_stats.by_type.is_empty() {
        summary.push_str(&format!("## 行动分布{nl}{nl}"));
        summary.push_str("江湖儿女们的日常活动如下：\n");

        let mut sorted_actions: Vec<_> = data.action_stats.by_type.iter().collect();
        sorted_actions.sort_by(|a, b| b.1.cmp(a.1));

        for (action, count) in sorted_actions.iter().take(5) {
            let cnt = **count;
            let percentage = (cnt as f64 / data.action_stats.total as f64 * 100.0) as i32;
            summary.push_str(&format!(
                "- {}: {} 次 ({}%)\n",
                action_type_display(action),
                cnt,
                percentage
            ));
        }
        summary.push_str(nl);
    }

    // 地点分布
    if !data.location_stats.is_empty() {
        summary.push_str(&format!("## 江湖格局{nl}{nl}"));
        summary.push_str("各据点的热闹程度：\n");

        for loc in data.location_stats.iter().take(5) {
            summary.push_str(&format!(
                "- {}: {} 次活动 ({:.1}%)\n",
                loc.location, loc.count, loc.percentage
            ));
        }
        summary.push_str(nl);
    }

    // 关键事件
    if !data.highlights.is_empty() {
        summary.push_str(&format!("## 本周大事{nl}{nl}"));

        // 按类型分组
        let deaths: Vec<_> = data
            .highlights
            .iter()
            .filter(|h| h.event_type == "death")
            .collect();
        let combats: Vec<_> = data
            .highlights
            .iter()
            .filter(|h| h.event_type == "combat")
            .collect();
        let dialogues: Vec<_> = data
            .highlights
            .iter()
            .filter(|h| h.event_type == "dialogue")
            .collect();
        let socials: Vec<_> = data
            .highlights
            .iter()
            .filter(|h| h.event_type == "social")
            .collect();

        let threshold = crate::chronicle::ChronicleConfig::default().highlight_threshold as usize;

        if !deaths.is_empty() {
            summary.push_str(&format!("### 生离死别{nl}{nl}"));
            for h in deaths.iter().take(threshold) {
                summary.push_str(&format!("- {}\n", h.description));
            }
            summary.push_str(nl);
        }

        if !combats.is_empty() {
            summary.push_str(&format!("### 刀光剑影{nl}{nl}"));
            for h in combats.iter().take(threshold) {
                summary.push_str(&format!("- {}\n", h.description));
            }
            summary.push_str(nl);
        }

        if !dialogues.is_empty() {
            summary.push_str(&format!("### 江湖传闻{nl}{nl}"));
            for h in dialogues.iter().take(threshold) {
                summary.push_str(&format!("- {}\n", h.description));
            }
            summary.push_str(nl);
        }

        if !socials.is_empty() {
            summary.push_str(&format!("### 人情往来{nl}{nl}"));
            for h in socials.iter().take(threshold) {
                summary.push_str(&format!("- {}\n", h.description));
            }
            summary.push_str(nl);
        }
    }

    // 人物群像
    if !data.agents.is_empty() {
        summary.push_str(&format!("## 江湖群像{nl}{nl}"));

        // 找出最活跃的 agent
        let mut sorted_agents: Vec<_> = data.agents.iter().collect();
        sorted_agents.sort_by(|a, b| b.actions_count.cmp(&a.actions_count));

        for agent in sorted_agents.iter().take(5) {
            summary.push_str(&format!("### {}\n\n", agent.name));
            summary.push_str(&format!("- 当前位置: {}\n", agent.location));
            summary.push_str(&format!("- 行动次数: {}\n", agent.actions_count));

            if !agent.top_actions.is_empty() {
                let top_strs: Vec<String> = agent
                    .top_actions
                    .iter()
                    .take(3)
                    .map(|(a, _)| action_type_display(a))
                    .collect();
                summary.push_str(&format!("- 主要活动: {}\n", top_strs.join("、")));
            }

            if agent.died_this_period {
                summary.push_str("- 命运: 陨落于本周期\n");
            }

            // 从叙事中提取一句作为代表
            if let Some(first_narrative) = agent.narratives.first() {
                let snippet = if first_narrative.len() > 50 {
                    let end = first_narrative
                        .char_indices()
                        .nth(47)
                        .map(|(idx, _)| idx)
                        .unwrap_or(first_narrative.len());
                    format!("{}...", &first_narrative[..end])
                } else {
                    first_narrative.clone()
                };
                summary.push_str(&format!("- 自述: \"{}\"\n", snippet));
            }

            summary.push_str(nl);
        }
    }

    // 结语
    summary.push_str(&format!("--{nl}{nl}"));
    summary.push_str(&format!(
        "第{}日至第{}日，{}季。江湖儿女们在这片天地间继续书写着属于自己的故事。\n",
        data.game_day_start, data.game_day_end, data.season
    ));

    Ok(summary)
}

/// LLM 生成（调用外部 LLM）
///
/// 配置方式：从 llm.yaml 配置文件读取
/// 添加超时和重试机制
pub async fn generate_llm(data: &CollectedData) -> Result<String> {
    // 从配置文件读取 LLM 配置
    let config = match crate::game_data::loaders::load_llm(&crate::paths::get_config_dir()) {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::warn!("LLM 配置加载失败: {}", e);
            anyhow::bail!("LLM 配置加载失败: {}", e);
        }
    };

    if !config.enabled {
        tracing::info!("LLM 生成已禁用，跳过");
        anyhow::bail!("LLM 生成已禁用");
    }

    if config.api_key.is_empty() {
        tracing::warn!("LLM API 密钥未设置");
        anyhow::bail!("LLM API 密钥未设置");
    }

    let prompt = build_llm_prompt(data);

    tracing::info!(
        "正在调用 LLM 生成群像传记 (provider: {}, model: {}, base_url: {})",
        config.provider,
        config.model,
        config.base_url
    );

    // 构建请求体
    let request_body = serde_json::json!({
        "model": config.model,
        "messages": [
            {
                "role": "system",
                "content": "你是一位武侠小说作家，擅长以古龙的笔法书写江湖故事。"
            },
            {
                "role": "user",
                "content": prompt
            }
        ],
        "max_tokens": config.max_tokens,
        "temperature": config.temperature
    });

    // 使用带超时的 client
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120)) // 120秒超时
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .context("构建 HTTP 客户端失败")?;

    let base_url = config.base_url.trim_end_matches('/');
    let url = if base_url.contains("/chat/completions") {
        base_url.to_string()
    } else {
        format!("{}/chat/completions", base_url)
    };

    tracing::debug!("LLM 请求 URL: {}", url);
    tracing::debug!("LLM 请求体: {}", request_body);

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.api_key))
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await
        .context(format!("LLM 请求失败 (URL: {})", url))?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();

    tracing::debug!("LLM 响应状态: {}, body: {}", status, body);

    if !status.is_success() {
        record_llm_error();
        anyhow::bail!("LLM 返回错误状态 {}: {}", status, body);
    }

    #[derive(serde::Deserialize)]
    struct LlmResponse {
        choices: Vec<Choice>,
        #[serde(default)]
        usage: Option<Usage>,
    }

    #[derive(serde::Deserialize)]
    struct Choice {
        message: Message,
    }

    #[derive(serde::Deserialize)]
    struct Message {
        content: String,
    }

    #[derive(serde::Deserialize)]
    struct Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
        #[serde(default)]
        #[allow(dead_code)]
        total_tokens: u32,
    }

    let llm_response: LlmResponse = serde_json::from_str(&body).context("解析 LLM 响应失败")?;

    let usage = llm_response.usage.as_ref();
    let input_tokens = usage.map(|u| u.prompt_tokens as u64).unwrap_or(0);
    let output_tokens = usage.map(|u| u.completion_tokens as u64).unwrap_or(0);

    // 记录 Token 使用
    if input_tokens > 0 || output_tokens > 0 {
        record_llm_tokens(input_tokens, output_tokens);
    }

    let content = llm_response
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default();

    if content.trim().is_empty() {
        record_llm_error();
        anyhow::bail!("LLM 返回空内容");
    }

    tracing::info!("LLM 群像传记生成完成");
    Ok(content)
}

/// 构建 LLM prompt
fn build_llm_prompt(data: &CollectedData) -> String {
    let mut prompt = String::new();
    let agent_count = data.agents.len() as i32;

    prompt.push_str(&format!(
        "请为以下江湖周期撰写一份群像传记（第{}日至第{}日，{}季）：\n\n",
        data.game_day_start, data.game_day_end, data.season
    ));

    prompt.push_str(&format!("- 参与人数: {} 人\n", agent_count));
    prompt.push_str(&format!("- 总行动数: {} 次\n", data.action_stats.total));
    prompt.push_str(&format!("- 死亡人数: {} 人\n", data.deaths));
    prompt.push_str(&format!("- 存活率: {:.1}%\n\n", {
        if agent_count > 0 {
            ((agent_count - data.deaths) as f64 / agent_count as f64) * 100.0
        } else {
            100.0
        }
    }));

    if !data.highlights.is_empty() {
        prompt.push_str("关键事件：\n");
        let threshold = crate::chronicle::ChronicleConfig::default().highlight_threshold as usize;
        for h in data.highlights.iter().take(threshold * 3) {
            prompt.push_str(&format!(
                "- [{}] {}\n",
                event_type_display(&h.event_type),
                h.description
            ));
        }
        prompt.push('\n');
    }

    if !data.agents.is_empty() {
        prompt.push_str("人物简报：\n");
        for agent in data.agents.iter().take(5) {
            prompt.push_str(&format!(
                "- {}（{}）：{}次行动，主要{}，{}\n",
                agent.name,
                agent.location,
                agent.actions_count,
                agent
                    .top_actions
                    .iter()
                    .take(2)
                    .map(|(a, _)| action_type_display(a))
                    .collect::<Vec<_>>()
                    .join("、"),
                if agent.died_this_period {
                    "已陨落"
                } else {
                    "尚在江湖"
                }
            ));
        }
        prompt.push('\n');
    }

    prompt.push_str(
        "请以武侠小说的笔法撰写这份群像传记，要求：\n\
         1. 语言古朴典雅，有古龙遗风\n\
         2. 每个重要人物都要有专属描述\n\
         3. 关键事件要有戏剧性描写\n\
         4. 结尾要有意境，留有余韵\n\
         5. 字数 800-1500 字\n",
    );

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chronicle::ActionStats;
    use crate::chronicle::collector::{AgentInfo, CollectedData};
    use std::collections::HashMap;

    #[test]
    fn test_action_type_display() {
        assert_eq!(action_type_display("休息"), "静修");
        assert_eq!(action_type_display("说话"), "交谈");
        assert_eq!(action_type_display("移动"), "行走");
        assert_eq!(action_type_display("攻击"), "战斗");
        assert_eq!(action_type_display("unknown"), "unknown");
    }

    #[test]
    fn test_template_generation() {
        let data = CollectedData {
            period_start: 1,
            period_end: 168,
            game_day_start: 1,
            game_day_end: 7,
            season: "春".to_string(),
            agents: vec![AgentInfo {
                agent_id: uuid::Uuid::new_v4(),
                name: "张三".to_string(),
                location: " village_center".to_string(),
                actions_count: 50,
                top_actions: vec![("移动".to_string(), 20), ("采集".to_string(), 15)],
                narratives: vec!["在江湖中行走，感受春风".to_string()],
                died_this_period: false,
            }],
            highlights: vec![],
            action_stats: ActionStats {
                total: 100,
                by_type: HashMap::from([
                    ("移动".to_string(), 40),
                    ("休息".to_string(), 30),
                    ("采集".to_string(), 30),
                ]),
                success_rate: 0.85,
            },
            location_stats: vec![],
            deaths: 2,
            births: 5,
        };

        let summary = generate_template(&data).unwrap();
        assert!(summary.contains("第1日至第7日"));
        assert!(summary.contains("春"));
        assert!(summary.contains("1 位江湖儿女"));
        assert!(summary.contains("100 次行动"));
    }
}
