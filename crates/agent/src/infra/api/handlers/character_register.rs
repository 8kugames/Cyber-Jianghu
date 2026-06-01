// 角色注册 API Handlers
// ============================================================================

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::config::{
    CharacterConfig, CharacterGenerationConfig, CharacterStatus, FieldConstraints, FieldSpec,
};

use super::HttpApiState;
use super::basic::ErrorResponse;
use super::character_helpers::{get_device_id, save_character};

/// 角色注册请求（从 CLI 接收）
#[derive(Debug, Deserialize)]
pub struct CharacterRegisterRequest {
    /// 角色姓名
    pub name: String,
    /// 年龄
    pub age: u8,
    /// 性别
    pub gender: String,
    /// 外貌描述
    #[serde(default)]
    pub appearance: Option<String>,
    /// 身份背景
    #[serde(default)]
    pub identity: Option<String>,
    /// 性格特征
    #[serde(default)]
    pub personality: Vec<String>,
    /// 核心价值观
    #[serde(default)]
    pub values: Vec<String>,
    /// 语言风格
    #[serde(default)]
    pub language_style: LanguageStyleRequest,
    /// 目标
    #[serde(default)]
    pub goals: GoalsRequest,
    /// 系统提示词（可选）
    #[serde(default)]
    pub system_prompt: Option<String>,
}

/// POST /api/v1/character/generate 请求体
#[derive(Debug, Deserialize, Default)]
pub(crate) struct GenerateCharacterRequest {
    /// 指定姓氏（可选，用于批量创建时保证多样性）
    #[serde(default)]
    pub surname_hint: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub(crate) struct LanguageStyleRequest {
    #[serde(default)]
    tone: Option<String>,
    #[serde(default)]
    speech_patterns: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub(crate) struct GoalsRequest {
    #[serde(default)]
    short_term: Option<String>,
    #[serde(default)]
    long_term: Option<String>,
}

// ============================================================================
// Schema-driven prompt generation + validation
// ============================================================================

/// Resolve dot-notation path in JSON value (e.g. "language_style.tone")
fn resolve_path<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

/// Schema-driven validation error
#[derive(Debug)]
struct FieldValidationError {
    path: String,
    message: String,
}

impl std::fmt::Display for FieldValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// Validate JSON value against field specs
fn validate_against_schema(
    value: &serde_json::Value,
    fields: &[FieldSpec],
) -> Result<(), Vec<FieldValidationError>> {
    let mut errors = Vec::new();

    for spec in fields {
        let field_val = resolve_path(value, &spec.path);

        match &spec.constraints {
            FieldConstraints::String {
                required,
                min_chars,
                max_chars,
                ..
            } => match field_val {
                None | Some(serde_json::Value::Null) => {
                    if *required {
                        errors.push(FieldValidationError {
                            path: spec.path.clone(),
                            message: "required field missing".into(),
                        });
                    }
                }
                Some(serde_json::Value::String(s)) => {
                    let len = s.chars().count();
                    if len < *min_chars {
                        errors.push(FieldValidationError {
                            path: spec.path.clone(),
                            message: format!("min {} chars, got {}", min_chars, len),
                        });
                    }
                    if len > *max_chars {
                        errors.push(FieldValidationError {
                            path: spec.path.clone(),
                            message: format!("max {} chars, got {}", max_chars, len),
                        });
                    }
                }
                Some(other) => {
                    errors.push(FieldValidationError {
                        path: spec.path.clone(),
                        message: format!("expected string, got {}", other),
                    });
                }
            },
            FieldConstraints::Integer { required, min, max } => match field_val {
                None | Some(serde_json::Value::Null) => {
                    if *required {
                        errors.push(FieldValidationError {
                            path: spec.path.clone(),
                            message: "required field missing".into(),
                        });
                    }
                }
                Some(serde_json::Value::Number(n)) => {
                    if let Some(n) = n.as_u64() {
                        let n = n as u32;
                        if n < *min || n > *max {
                            errors.push(FieldValidationError {
                                path: spec.path.clone(),
                                message: format!("must be {}-{}, got {}", min, max, n),
                            });
                        }
                    } else {
                        errors.push(FieldValidationError {
                            path: spec.path.clone(),
                            message: "expected integer".into(),
                        });
                    }
                }
                Some(other) => {
                    errors.push(FieldValidationError {
                        path: spec.path.clone(),
                        message: format!("expected integer, got {}", other),
                    });
                }
            },
            FieldConstraints::Enum {
                required, options, ..
            } => match field_val {
                None | Some(serde_json::Value::Null) => {
                    if *required {
                        errors.push(FieldValidationError {
                            path: spec.path.clone(),
                            message: "required field missing".into(),
                        });
                    }
                }
                Some(serde_json::Value::String(s)) => {
                    if !options.contains(s) {
                        errors.push(FieldValidationError {
                            path: spec.path.clone(),
                            message: format!(
                                "invalid: \"{}\" (allowed: {})",
                                s,
                                options.join("\u{3001}")
                            ),
                        });
                    }
                }
                Some(other) => {
                    errors.push(FieldValidationError {
                        path: spec.path.clone(),
                        message: format!("expected string, got {}", other),
                    });
                }
            },
            FieldConstraints::EnumArray {
                required,
                options,
                min_count,
                max_count,
                ..
            } => match field_val {
                None | Some(serde_json::Value::Null) => {
                    if *required {
                        errors.push(FieldValidationError {
                            path: spec.path.clone(),
                            message: "required field missing".into(),
                        });
                    }
                }
                Some(serde_json::Value::String(s)) => {
                    if !options.contains(s) {
                        errors.push(FieldValidationError {
                            path: spec.path.clone(),
                            message: format!(
                                "invalid: \"{}\" (allowed: {})",
                                s,
                                options.join("\u{3001}")
                            ),
                        });
                    }
                }
                Some(serde_json::Value::Array(arr)) => {
                    if arr.len() < *min_count || arr.len() > *max_count {
                        errors.push(FieldValidationError {
                            path: spec.path.clone(),
                            message: format!(
                                "need {}-{} items, got {}",
                                min_count,
                                max_count,
                                arr.len()
                            ),
                        });
                    }
                    let invalid: Vec<String> = arr
                        .iter()
                        .filter_map(|v| v.as_str())
                        .filter(|s| !options.iter().any(|o| o == *s))
                        .map(|s| s.to_string())
                        .collect();
                    if !invalid.is_empty() {
                        errors.push(FieldValidationError {
                            path: spec.path.clone(),
                            message: format!(
                                "invalid: {} (allowed: {})",
                                invalid.join("\u{3001}"),
                                options.join("\u{3001}")
                            ),
                        });
                    }
                }
                Some(other) => {
                    errors.push(FieldValidationError {
                        path: spec.path.clone(),
                        message: format!("expected array, got {}", other),
                    });
                }
            },
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Resolve template variables in prompt_text from field constraints
fn resolve_prompt_template(
    template: &str,
    spec: &FieldSpec,
    extra_vars: &std::collections::HashMap<String, String>,
) -> String {
    let mut result = template.to_string();
    match &spec.constraints {
        FieldConstraints::String {
            min_chars,
            max_chars,
            ..
        } => {
            result = result.replace("{min_chars}", &min_chars.to_string());
            result = result.replace("{max_chars}", &max_chars.to_string());
        }
        FieldConstraints::Integer { min, max, .. } => {
            result = result.replace("{min}", &min.to_string());
            result = result.replace("{max}", &max.to_string());
        }
        FieldConstraints::Enum { options, .. } => {
            result = result.replace("{options}", &options.join("\u{3001}"));
        }
        FieldConstraints::EnumArray {
            options,
            min_count,
            max_count,
            ..
        } => {
            result = result.replace("{options}", &options.join("\u{3001}"));
            result = result.replace("{min_count}", &min_count.to_string());
            result = result.replace("{max_count}", &max_count.to_string());
        }
    }
    for (k, v) in extra_vars {
        result = result.replace(&format!("{{{}}}", k), v);
    }
    result
}

/// Generate prompt field line from a single field spec
fn generate_field_line(
    spec: &FieldSpec,
    extra_vars: &std::collections::HashMap<String, String>,
) -> String {
    let field_name = spec.path.split('.').next_back().unwrap_or(&spec.path);

    // Check for prompt_text override
    let prompt_text = match &spec.constraints {
        FieldConstraints::String {
            prompt_text: Some(txt),
            ..
        }
        | FieldConstraints::Enum {
            prompt_text: Some(txt),
            ..
        } => Some(txt.clone()),
        _ => None,
    };

    if let Some(txt) = prompt_text {
        let resolved = resolve_prompt_template(&txt, spec, extra_vars);
        return format!("- {}: {}", field_name, resolved);
    }

    // Auto-generate from constraints
    match &spec.constraints {
        FieldConstraints::String {
            max_chars,
            min_chars,
            ..
        } => {
            if *min_chars > 0 {
                format!("- {}: {}-{} chars", field_name, min_chars, max_chars)
            } else if *max_chars > 0 {
                format!("- {}: max {} chars", field_name, max_chars)
            } else {
                format!("- {}: string", field_name)
            }
        }
        FieldConstraints::Integer { min, max, .. } => {
            format!("- {}: {}-{} (integer)", field_name, min, max)
        }
        FieldConstraints::Enum { options, .. } => {
            format!(
                "- {}: pick 1 from: {}",
                field_name,
                options.join("\u{3001}")
            )
        }
        FieldConstraints::EnumArray {
            options,
            min_count,
            max_count,
            extra_prompt,
            ..
        } => {
            let base = format!(
                "- {}: pick {}-{} from: {}",
                field_name,
                min_count,
                max_count,
                options.join("\u{3001}")
            );
            if let Some(extra) = extra_prompt {
                format!("{}, {}", base, extra)
            } else {
                base
            }
        }
    }
}

/// Build full character generation prompt from schema
fn generate_character_prompt(
    cg: &CharacterGenerationConfig,
    extra_vars: &std::collections::HashMap<String, String>,
) -> String {
    let mut top_level = Vec::new();
    let mut groups: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();

    for spec in &cg.fields {
        let line = generate_field_line(spec, extra_vars);
        if let Some((parent, _)) = spec.path.split_once('.') {
            groups.entry(parent.to_string()).or_default().push(line);
        } else {
            top_level.push(line);
        }
    }

    let mut field_section = String::new();
    for line in &top_level {
        field_section.push_str(line);
        field_section.push('\n');
    }
    for (parent, field_lines) in &groups {
        field_section.push_str(&format!("- {}: object:\n", parent));
        for line in field_lines {
            field_section.push_str(&format!("  {}\n", line));
        }
    }

    format!(
        r#"Generate a character fitting this world:

## World
{world_setting}

## Core Requirements
1. **Diversity**: distinct from typical characters in background, personality, values, speech
2. **Authenticity**: complex motivations, unique speech patterns

## Field Requirements
{field_section}## Output Format
Strict JSON output, no other text."#,
        world_setting = cg.world_setting,
        field_section = field_section,
    )
}

#[derive(Debug, Serialize)]
pub struct CharacterRegisterResponse {
    /// 角色 ID（服务器分配）
    pub agent_id: String,
    /// 结果消息
    pub message: String,
    /// 警告信息（如配置保存失败）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// LLM 生成角色处理器
///
/// POST /api/v1/character/generate - 使用 LLM 自动生成角色
///
/// 可选 JSON body: `{ "surname_hint": "柳" }` — 指定姓氏，用于批量创建时保证多样性
pub(crate) async fn generate_character_handler(
    State(state): State<HttpApiState>,
    body: Option<Json<GenerateCharacterRequest>>,
) -> impl IntoResponse {
    let body = body.map(|b| b.0).unwrap_or_default();

    // 1. 读取配置文件
    let config = match crate::config::Config::from_file(&state.config_path) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error_code: "config_read_error".to_string(),
                    message: format!("读取配置文件失败: {}", e),
                }),
            )
                .into_response();
        }
    };

    // 2. 获取共享 LLM 客户端（模式无关：Cognitive 用 FallbackLlmClient，Claw 用 OpenClawBridge）
    let llm_client: std::sync::Arc<dyn crate::component::llm::LlmClient> = {
        let guard = state.llm_container.read().await;
        match guard.as_ref() {
            Some(container) => container.read().await.clone(),
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(ErrorResponse {
                        error_code: "llm_not_initialized".to_string(),
                        message: "LLM 未初始化，请先配置 LLM".to_string(),
                    }),
                )
                    .into_response();
            }
        }
    };

    // 4. 构建角色生成 prompt
    // 姓氏约束策略（纯 prompt 工程，零计数）：
    //   - surname_hint 指定时 → 直接约束姓氏
    //   - 未指定时 → 提示从百家姓中自由选取，不要求计数到第 N 个
    let surname_constraint = match &body.surname_hint {
        Some(hint) => format!("姓氏必须为\"{}\"", hint),
        None => "姓氏需从百家姓中选取".to_string(),
    };
    let mut extra_vars = std::collections::HashMap::new();
    extra_vars.insert("surname_constraint".into(), surname_constraint);
    let prompt = generate_character_prompt(&config.character_generation, &extra_vars);

    #[derive(Debug, serde::Deserialize, serde::Serialize)]
    struct GeneratedCharacter {
        name: String,
        age: u8,
        gender: String,
        appearance: Option<String>,
        identity: Option<String>,
        personality: Vec<String>,
        values: Vec<String>,
        language_style: LanguageStyleRequest,
        goals: GoalsRequest,
    }

    // 3. 使用 per-call config 覆盖 temperature（角色生成是创意任务）
    use crate::component::llm::LlmClientExt;
    let chat_config = crate::component::llm::ChatExchangeConfig {
        model: llm_client.model_name(),
        temperature: 0.9,
        max_tokens: 2048,
        enable_thinking: None,
    };
    match llm_client
        .complete_json_with_config::<serde_json::Value>(&prompt, chat_config)
        .await
    {
        Ok(json_value) => {
            // Schema validation before deserialization
            if let Err(errors) =
                validate_against_schema(&json_value, &config.character_generation.fields)
            {
                let msgs: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
                warn!(
                    "[character] LLM output validation failed: {}",
                    msgs.join("; ")
                );
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(ErrorResponse {
                        error_code: "validation_failed".to_string(),
                        message: format!("Role validation failed: {}", msgs.join("; ")),
                    }),
                )
                    .into_response();
            }
            match serde_json::from_value::<GeneratedCharacter>(json_value) {
                Ok(character) => {
                    info!("[character] LLM generate success: {}", character.name);
                    (StatusCode::OK, Json(character)).into_response()
                }
                Err(e) => {
                    error!("[character] JSON deserialization failed: {}", e);
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(ErrorResponse {
                            error_code: "parse_failed".to_string(),
                            message: format!("Parse failed: {}", e),
                        }),
                    )
                        .into_response()
                }
            }
        }
        Err(e) => {
            error!("[character] LLM 生成角色失败: {}", e);
            (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error_code: "generation_failed".to_string(),
                    message: format!("角色生成失败，请重试: {}", e),
                }),
            )
                .into_response()
        }
    }
}

/// 角色注册处理器
///
/// POST /api/v1/character/register - 创建新角色
///
/// 接收 CLI 的角色创建请求，添加设备认证信息后转发到 Server
pub(crate) async fn register_character_handler(
    State(state): State<HttpApiState>,
    Json(value): Json<serde_json::Value>,
) -> impl IntoResponse {
    use reqwest::Client;
    use tracing::info;

    // 1. 检查设备身份
    let (device_id, auth_token) = match get_device_id(&state).await {
        Ok(id) => id,
        Err(e) => {
            error!("设备身份未初始化: {}", e);
            return (
                StatusCode::PRECONDITION_FAILED,
                Json(CharacterRegisterResponse {
                    agent_id: String::new(),
                    message: "设备身份未初始化，请先启动 Agent".to_string(),
                    warning: None,
                }),
            )
                .into_response();
        }
    };

    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    info!("角色注册请求: {}", name);

    // 2. Load config + schema validation
    let config = match crate::config::Config::from_file(&state.config_path) {
        Ok(c) => c,
        Err(e) => {
            error!("读取配置文件失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(CharacterRegisterResponse {
                    agent_id: String::new(),
                    message: format!("读取配置文件失败: {}", e),
                    warning: None,
                }),
            )
                .into_response();
        }
    };

    // 3. Schema validation
    if let Err(errors) = validate_against_schema(&value, &config.character_generation.fields) {
        let msgs: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
        warn!("角色注册参数验证失败: {}", msgs.join("; "));
        return (
            StatusCode::BAD_REQUEST,
            Json(CharacterRegisterResponse {
                agent_id: String::new(),
                message: msgs.join("; "),
                warning: None,
            }),
        )
            .into_response();
    }

    // 4. Deserialize into typed struct
    let payload: CharacterRegisterRequest = match serde_json::from_value(value) {
        Ok(p) => p,
        Err(e) => {
            warn!("JSON 反序列化失败: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(CharacterRegisterResponse {
                    agent_id: String::new(),
                    message: format!("请求格式错误: {}", e),
                    warning: None,
                }),
            )
                .into_response();
        }
    };

    // 5. Generate default system_prompt
    let system_prompt = payload.system_prompt.clone().unwrap_or_else(|| {
        format!(
            "你是{}，{}岁，{}。{}{}你的目标是探索这个江湖世界，与各路侠客交流，并在武林中闯出自己的一片天地。",
            payload.name,
            payload.age,
            payload.identity.as_deref().unwrap_or("江湖中人"),
            payload.appearance.as_deref().map(|a| a.to_string()).unwrap_or_default(),
            if !payload.personality.is_empty() {
                format!("性格特点：{}。", payload.personality.join("、"))
            } else {
                String::new()
            }
        )
    });

    // 6. 构建发送到 Server 的请求
    let server_request = serde_json::json!({
        "device_id": device_id,
        "auth_token": auth_token,
        "name": payload.name,
        "age": payload.age,
        "gender": payload.gender,
        "appearance": payload.appearance,
        "identity": payload.identity,
        "personality": payload.personality,
        "values": payload.values,
        "language_style": payload.language_style,
        "goals": payload.goals,
        "system_prompt": system_prompt,
    });

    // 7. 转发到 Server
    let client = Client::new();
    let server_http_url = state.server_http_url.read().await.clone();
    let server_url = format!("{}/api/v1/agent/register", server_http_url);

    let mut response = match client.post(&server_url).json(&server_request).send().await {
        Ok(resp) => resp,
        Err(e) => {
            error!("连接服务器失败: {}", e);
            return (
                StatusCode::BAD_GATEWAY,
                Json(CharacterRegisterResponse {
                    agent_id: String::new(),
                    message: format!("连接服务器失败: {}", e),
                    warning: None,
                }),
            )
                .into_response();
        }
    };

    // 8. 处理 Server 响应
    if !response.status().is_success() {
        let status = response.status();

        if status == StatusCode::UNAUTHORIZED {
            warn!("收到 401，尝试刷新令牌后重试...");
            if let Err(e) = state.refresh_auth_token().await {
                error!("刷新令牌失败: {}", e);
                let _body = response.text().await.unwrap_or_default();
                return (
                    status,
                    Json(CharacterRegisterResponse {
                        agent_id: String::new(),
                        message: format!("认证失败且刷新令牌失败: {}", e),
                        warning: None,
                    }),
                )
                    .into_response();
            }

            let (device_id, auth_token) = match get_device_id(&state).await {
                Ok(id) => id,
                Err(e) => {
                    error!("刷新令牌后获取设备ID失败: {}", e);
                    let _body = response.text().await.unwrap_or_default();
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(CharacterRegisterResponse {
                            agent_id: String::new(),
                            message: format!("刷新令牌后获取设备ID失败: {}", e),
                            warning: None,
                        }),
                    )
                        .into_response();
                }
            };

            let server_request = serde_json::json!({
                "device_id": device_id,
                "auth_token": auth_token,
                "name": payload.name,
                "age": payload.age,
                "gender": payload.gender,
                "appearance": payload.appearance,
                "identity": payload.identity,
                "personality": payload.personality,
                "values": payload.values,
                "language_style": payload.language_style,
                "goals": payload.goals,
                "system_prompt": system_prompt,
            });

            let retry_response = match client.post(&server_url).json(&server_request).send().await {
                Ok(resp) => resp,
                Err(e) => {
                    error!("重试连接服务器失败: {}", e);
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(CharacterRegisterResponse {
                            agent_id: String::new(),
                            message: format!("连接服务器失败: {}", e),
                            warning: None,
                        }),
                    )
                        .into_response();
                }
            };

            if !retry_response.status().is_success() {
                let status = retry_response.status();
                let body = retry_response.text().await.unwrap_or_default();
                error!("重试后服务器拒绝注册: {} - {}", status, body);
                return (
                    status,
                    Json(CharacterRegisterResponse {
                        agent_id: String::new(),
                        message: format!("服务器拒绝: {}", body),
                        warning: None,
                    }),
                )
                    .into_response();
            }

            response = retry_response;
        } else {
            let body = response.text().await.unwrap_or_default();
            error!("服务器拒绝注册: {} - {}", status, body);
            return (
                status,
                Json(CharacterRegisterResponse {
                    agent_id: String::new(),
                    message: format!("服务器拒绝: {}", body),
                    warning: None,
                }),
            )
                .into_response();
        }
    }

    // 9. 解析成功响应
    #[derive(Deserialize)]
    struct ServerRegisterResponse {
        agent_id: String,
        message: String,
        #[allow(dead_code)]
        game_rules: Option<cyber_jianghu_protocol::GameRules>,
        narrative_config: Option<cyber_jianghu_protocol::NarrativeConfig>,
        narrative_config_hash: Option<String>,
        #[serde(default)]
        initial_attributes: std::collections::HashMap<String, i32>,
    }

    match response.json::<ServerRegisterResponse>().await {
        Ok(result) => {
            info!("角色注册成功: {} -> {}", payload.name, result.agent_id);

            // 10. 保存 narrative_config 到本地配置目录（hash skip-optimization）
            if let Some(ref narrative_config) = result.narrative_config {
                // 内存始终更新
                *state.narrative_config.write().await = result.narrative_config.clone();

                let hash = result.narrative_config_hash.as_deref();
                if let Err(e) = crate::config::save_narrative_config_to_disk(
                    narrative_config, hash,
                ) {
                    error!("保存 narrative_config 失败: {}", e);
                }
            }

            // 11. 创建并保存角色配置到文件系统
            let mut config_warning = None;
            let agent_uuid = match uuid::Uuid::parse_str(&result.agent_id) {
                Ok(id) => id,
                Err(e) => {
                    error!("解析 agent_id 失败: {}", e);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(CharacterRegisterResponse {
                            agent_id: String::new(),
                            message: format!("解析 agent_id 失败: {}", e),
                            warning: None,
                        }),
                    )
                        .into_response();
                }
            };

            let new_character = CharacterConfig {
                agent_id: Some(agent_uuid),
                name: payload.name.clone(),
                age: payload.age,
                gender: payload.gender.clone(),
                appearance: payload.appearance.clone(),
                identity: payload.identity.clone(),
                personality: payload.personality.clone(),
                values: payload.values.clone(),
                language_style: crate::config::LanguageStyleConfig {
                    tone: payload.language_style.tone.clone(),
                    speech_patterns: payload.language_style.speech_patterns.clone(),
                },
                goals: crate::config::GoalsConfig {
                    short_term: payload.goals.short_term.clone(),
                    long_term: payload.goals.long_term.clone(),
                },
                system_prompt: Some(system_prompt.clone()),
                registered_at: Some(chrono::Utc::now()),
                birth_attributes: if result.initial_attributes.is_empty() {
                    None
                } else {
                    Some(result.initial_attributes.clone())
                },
                status: CharacterStatus::Alive,
                server_url: Some(server_http_url.clone()),
                last_connected_real_time: None,
                last_connected_world_time: None,
                biography: None,
            };

            if let Err(e) = save_character(&new_character, &state.character_dir.read().await) {
                error!("保存角色配置失败: {}", e);
                config_warning = Some(format!("角色配置保存失败: {}", e));
            }

            // 12. 更新运行时 agent_id（使后续 Intent 提交使用新角色）
            {
                let mut id = state.agent_id.write().await;
                *id = agent_uuid;
                info!(
                    "[character] Updated runtime agent_id to {} ({})",
                    agent_uuid, payload.name
                );
            }

            // 13. 重置死亡状态（新角色 = 新生命）
            state
                .is_dead
                .store(false, std::sync::atomic::Ordering::Relaxed);

            // 14. 触发 WebSocket 重连以注册新角色
            if let Some(ref tx) = state.reconnect_tx {
                let server_ws_url = state.server_ws_url.read().await.clone();
                let reconnect_req = crate::infra::api::ReconnectRequest {
                    ws_url: server_ws_url,
                    agent_id: Some(agent_uuid),
                };
                if let Err(e) = tx.send(reconnect_req) {
                    error!("[character] 注册后触发重连失败: {}", e);
                } else {
                    info!("[character] 注册后触发 WebSocket 重连");
                }
            }

            (
                StatusCode::OK,
                Json(CharacterRegisterResponse {
                    agent_id: result.agent_id,
                    message: result.message,
                    warning: config_warning,
                }),
            )
                .into_response()
        }
        Err(e) => {
            error!("解析服务器响应失败: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(CharacterRegisterResponse {
                    agent_id: String::new(),
                    message: format!("解析响应失败: {}", e),
                    warning: None,
                }),
            )
                .into_response()
        }
    }
}

// ============================================================================
