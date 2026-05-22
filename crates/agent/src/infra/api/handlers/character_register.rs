// 角色注册 API Handlers
// ============================================================================


use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::config::{CharacterConfig, CharacterStatus};


use super::HttpApiState;
use super::character_helpers::{get_device_id, save_character};
use super::basic::ErrorResponse;

/// 角色注册请求（从 CLI 接收）
#[derive(Debug, Deserialize)]
pub struct CharacterRegisterRequest {
    /// 角色姓名
    pub name: String,
    /// 年龄
    #[serde(default = "default_age")]
    pub age: u8,
    /// 性别
    #[serde(default = "default_gender")]
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

fn default_age() -> u8 {
    25
}
fn default_gender() -> String {
    "男".to_string()
}

/// 角色注册验证错误
#[derive(Debug)]
enum CharacterRegisterValidationError {
    NameEmpty,
    NameTooLong(usize),
    AgeOutOfRange(u8),
    InvalidGender(String),
    IdentityTooLong(usize),
    ShortTermGoalTooLong(usize),
    LongTermGoalTooLong(usize),
}

impl std::fmt::Display for CharacterRegisterValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NameEmpty => write!(f, "角色姓名不能为空"),
            Self::NameTooLong(len) => write!(f, "角色姓名不能超过20字符（当前{}字符）", len),
            Self::AgeOutOfRange(age) => write!(f, "年龄必须在1-100之间（当前{}）", age),
            Self::InvalidGender(g) => write!(f, "性别仅允许“男”或“女”（当前：“{}”）", g),
            Self::IdentityTooLong(len) => write!(f, "身份背景不能超过300字符（当前{}字符）", len),
            Self::ShortTermGoalTooLong(len) => {
                write!(f, "短期目标不能超过100字符（当前{}字符）", len)
            }
            Self::LongTermGoalTooLong(len) => {
                write!(f, "长远目标不能超过100字符（当前{}字符）", len)
            }
        }
    }
}

impl CharacterRegisterRequest {
    /// 验证请求参数是否符合前端输入框约束
    fn validate(&self) -> Result<(), CharacterRegisterValidationError> {
        // 姓名：必填，最大20字符
        if self.name.trim().is_empty() {
            return Err(CharacterRegisterValidationError::NameEmpty);
        }
        if self.name.chars().count() > 20 {
            return Err(CharacterRegisterValidationError::NameTooLong(
                self.name.chars().count(),
            ));
        }

        // 年龄：1-100
        if self.age < 1 || self.age > 100 {
            return Err(CharacterRegisterValidationError::AgeOutOfRange(self.age));
        }

        // 性别：仅允许"男"或"女"
        if self.gender != "男" && self.gender != "女" {
            return Err(CharacterRegisterValidationError::InvalidGender(
                self.gender.clone(),
            ));
        }

        // 身份背景：最大300字符
        if let Some(ref identity) = self.identity
            && identity.chars().count() > 300
        {
            return Err(CharacterRegisterValidationError::IdentityTooLong(
                identity.chars().count(),
            ));
        }

        // 短期目标：最大100字符
        if let Some(ref short_term) = self.goals.short_term
            && short_term.chars().count() > 100
        {
            return Err(CharacterRegisterValidationError::ShortTermGoalTooLong(
                short_term.chars().count(),
            ));
        }

        // 长远目标：最大100字符
        if let Some(ref long_term) = self.goals.long_term
            && long_term.chars().count() > 100
        {
            return Err(CharacterRegisterValidationError::LongTermGoalTooLong(
                long_term.chars().count(),
            ));
        }

        Ok(())
    }
}

/// 角色注册响应（返回给 CLI）
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
/// 调用配置的 LLM 生成一个符合世界观的武侠角色，返回完整角色信息供用户确认
pub(crate) async fn generate_character_handler(
    State(state): State<HttpApiState>,
) -> impl IntoResponse {
    use crate::component::llm::{
        DirectLlmClient, DirectLlmClientConfig, LlmClientExt, LlmProvider,
    };

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

    // 2. 检查 LLM 是否已配置
    if config.llm.model.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error_code: "llm_not_configured".to_string(),
                message: "请先配置 LLM".to_string(),
            }),
        )
            .into_response();
    }

    // 3. 创建 LLM 客户端
    let provider = match LlmProvider::parse(&config.llm.provider) {
        Some(p) => p,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error_code: "invalid_provider".to_string(),
                    message: format!("不支持的 LLM Provider: {}", config.llm.provider),
                }),
            )
                .into_response();
        }
    };

    let mut client_config = DirectLlmClientConfig::new(provider, config.llm.api_key.as_deref());

    if let Some(ref model) = config.llm.model {
        client_config = client_config.with_model(model);
    }
    if let Some(ref base_url) = config.llm.base_url {
        client_config = client_config.with_base_url(base_url);
    }
    client_config = client_config.with_temperature(0.9);

    let llm_client = match DirectLlmClient::new(client_config) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error_code: "llm_client_error".to_string(),
                    message: format!("创建 LLM 客户端失败: {}", e),
                }),
            )
                .into_response();
        }
    };

    // 4. 构建角色生成 prompt
    let prompt = r#"你是一个武侠角色生成器。请生成一个符合以下世界观的角色：

## 世界观
时代：武侠架空世界，冷兵器时代。世界使用独立"天道历"纪年，与现实朝代无关。
允许的概念：内力、轻功、武功、点穴，暗器、毒术、医术、易容、阵法。
禁止的概念：魔法、仙术、法术、热武器、现代科技、超能力、穿越。

## 核心要求
1. **多样性**：生成的每个角色必须在姓名、身份背景、性格、价值观、语言风格、目标等方面与常见角色有明显差异
2. **避免重复**：不要生成重复或相似的角色，不同角色应该有截然不同的背景故事和个性
3. **真实性**：角色应该像一个真实的人，有复杂的动机和独特的说话方式

## 字段要求
- name: 姓名（2-6个汉字）
- age: 年龄（16-60的整数）
- gender: 性别（"男"或"女"）
- appearance: 外貌描述（20-50字），要有特色
- identity: 身份背景（如"江湖游侠"、"药铺掌柜"，不超过300字），要有独特的故事
- personality: 性格特征数组（从以下选项中选2-4个：豪爽、沉稳、机智、冷漠、善良、阴险、正义、贪婪、忠诚、狡猾），避免只选正面或只选负面，性格特征需要与身份背景吻合。
- values: 核心价值观数组（从以下选项中选1-3个：侠义、财富、权力、自由、荣誉、知识、爱情、友情、复仇、和平），核心价值观需要与身份背景吻合。
- language_style: 对象，包含：
  - tone: 语调（从以下选项中选1个：豪迈、温和、冷漠、狡黠、文雅）
  - speech_patterns: 说话特点数组（从以下选项中选1-3个：喜欢引用古诗词、说话简洁、喜欢用成语、说话带方言、喜欢开玩笑、说话谨慎）
- goals: 对象，包含：
  - short_term: 短期目标（不超过100字），要具体且有个人特色
  - long_term: 长远目标（不超过100字），要有野心或深度

## 输出格式
请严格输出 JSON，不要包含其他文字。"#;

    // 5. 调用 LLM 生成角色
    #[derive(Debug, Serialize, Deserialize)]
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

    match llm_client.complete_json::<GeneratedCharacter>(prompt).await {
        Ok(character) => {
            info!("[character] LLM 生成角色成功: {}", character.name);
            (StatusCode::OK, Json(character)).into_response()
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
    Json(payload): Json<CharacterRegisterRequest>,
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

    info!("角色注册请求: {}", payload.name);

    // 2. 验证前端输入约束
    if let Err(e) = payload.validate() {
        warn!("角色注册参数验证失败: {}", e);
        return (
            StatusCode::BAD_REQUEST,
            Json(CharacterRegisterResponse {
                agent_id: String::new(),
                message: e.to_string(),
                warning: None,
            }),
        )
            .into_response();
    }

    // 3. 生成默认 system_prompt（如果未提供）
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

    // 4. 构建发送到 Server 的请求
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

    // 5. 转发到 Server
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

    // 5. 处理 Server 响应
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

    // 6. 解析成功响应
    #[derive(Deserialize)]
    struct ServerRegisterResponse {
        agent_id: String,
        message: String,
        #[allow(dead_code)]
        game_rules: Option<cyber_jianghu_protocol::GameRules>,
        narrative_config: Option<cyber_jianghu_protocol::NarrativeConfig>,
        #[serde(default)]
        initial_attributes: std::collections::HashMap<String, i32>,
    }

    match response.json::<ServerRegisterResponse>().await {
        Ok(result) => {
            info!("角色注册成功: {} -> {}", payload.name, result.agent_id);

            // 7. 保存 narrative_config 到本地配置目录
            if let Some(ref narrative_config) = result.narrative_config
                && let Some(home) = dirs::home_dir()
            {
                let config_dir = home.join(".cyber-jianghu").join("config");
                if let Err(e) = std::fs::create_dir_all(&config_dir) {
                    error!("创建配置目录失败: {}", e);
                } else {
                    let config_path = config_dir.join("narrative_config.json");
                    match serde_json::to_string_pretty(narrative_config) {
                        Ok(json) => {
                            if let Err(e) = std::fs::write(&config_path, json) {
                                error!("保存 narrative_config 失败: {}", e);
                            } else {
                                info!("已保存 narrative_config 到 {:?}", config_path);
                                // 同步更新内存中的 narrative_config，避免重启后数据不一致
                                *state.narrative_config.write().await =
                                    result.narrative_config.clone();
                            }
                        }
                        Err(e) => error!("序列化 narrative_config 失败: {}", e),
                    }
                }
            }

            // 8. 创建并保存角色配置到文件系统
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

            // 9. 更新运行时 agent_id（使后续 Intent 提交使用新角色）
            {
                let mut id = state.agent_id.write().await;
                *id = agent_uuid;
                info!(
                    "[character] Updated runtime agent_id to {} ({})",
                    agent_uuid, payload.name
                );
            }

            // 10. 重置死亡状态（新角色 = 新生命）
            state
                .is_dead
                .store(false, std::sync::atomic::Ordering::Relaxed);

            // 11. 触发 WebSocket 重连以注册新角色
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
