//! 启动引导：配置加载/设备身份/角色选择/横幅/日志初始化

use super::*;

pub(crate) fn config_path() -> PathBuf {
    cyber_jianghu_agent::config::config_dir().join("agent.yaml")
}

// ============================================================================
// 配置加载与保存
// ============================================================================

pub(crate) fn load_config() -> Result<Option<Config>> {
    let path = config_path();
    if path.exists() {
        info!("加载配置: {}", path.display());
        let config = Config::from_file(&path).context("Failed to load config")?;
        Ok(Some(config))
    } else {
        Ok(None)
    }
}

pub(crate) fn save_config(config: &Config) -> Result<()> {
    let path = config_path();

    // 创建目录
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    config.save_to_file(&path)?;
    info!("配置已保存到: {}", path.display());
    Ok(())
}

// ============================================================================
// 角色注册 API
// ============================================================================

/// 角色注册响应（从 Agent API 返回）
#[derive(Debug, Deserialize)]
struct CharacterRegisterResponse {
    agent_id: String,
    message: String,
}

/// 通过 Agent API 创建角色
///
/// 将角色配置发送到 Agent HTTP API，由 Agent API 添加设备认证后转发到 Server
pub(crate) async fn create_character_via_api(
    agent_port: u16,
    character: CharacterConfig,
) -> Result<Uuid> {
    let client = Client::new();
    let url = format!("http://localhost:{}/api/v1/character/register", agent_port);

    info!("创建角色: {} -> {}", character.name, url);

    let response = client
        .post(&url)
        .json(&character)
        .send()
        .await
        .context("Failed to create character")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to create character: {} - {}", status, body);
    }

    let result: CharacterRegisterResponse = response
        .json()
        .await
        .context("Failed to parse character response")?;

    info!("角色创建成功: {}", result.message);
    Uuid::parse_str(&result.agent_id).context("Failed to parse agent_id as UUID")
}

// ============================================================================
// 确保设备身份存在（server-scoped）— 设备身份生命周期 v2
// ============================================================================
//
// 关键不变量：本地 device.yaml 中持有的 device_id 必须与 server 端认可的一致。
// 流程：
// 1. device.yaml 不存在 → 直接调 /device/register 申报
// 2. device.yaml 存在 → 调 /device/verify 严格校验
//    - 200 → 用 server 返回的 token 刷新本地（以 server 为准）
//    - 404 → 本地 yaml 是 stale（DB 被清空等），删除并 fall through 到分支 1
// ============================================================================

pub(crate) async fn ensure_device(config: &Config, ws_url: &str) -> Result<DeviceConfig> {
    let device_path = config.device_yaml_path(ws_url);
    let http_url = cyber_jianghu_agent::config::ws_to_http_url(ws_url);
    let client = reqwest::Client::new();

    if device_path.exists() {
        let local = DeviceConfig::from_file(&device_path)?;
        info!(
            "本地有 device.yaml，调用 /device/verify 校验 server 是否仍认可 {}",
            local.device_id
        );

        let resp = client
            .post(format!("{}/api/v1/device/verify", http_url))
            .json(&serde_json::json!({"device_id": local.device_id.to_string()}))
            .send()
            .await
            .context("调用 /device/verify 失败")?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            // server 不认这个 device → 本地 yaml 已 stale
            // 关键：必须先成功删除本地 yaml，再 fall through 到 register 分支
            // 否则 register 会用 server 新生成的 device_id 创建新 yaml，而旧的
            // 还在磁盘上 — 下次启动会再次触发 404，形成可复现死循环
            warn!(
                "server 不认可 device {}（404），删除本地 yaml 并重新申报",
                local.device_id
            );
            std::fs::remove_file(&device_path)
                .with_context(|| format!("删除 stale device.yaml 失败: {:?}", device_path))?;
            // fall through 到"无本地记录"分支
        } else if !resp.status().is_success() {
            // 网络错误 / 5xx 等：直接抛错，不假装通过也不删除
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("/device/verify 失败: HTTP {} - {}", status, body);
        } else {
            // 200：server 认可 → 用 server 返回的 token 替换本地（server 权威）
            let body: serde_json::Value = resp
                .json()
                .await
                .context("Failed to parse /device/verify response")?;
            let server_token = body["auth_token"]
                .as_str()
                .context("/device/verify 响应缺少 auth_token")?
                .to_string();

            let refreshed = DeviceConfig {
                device_id: local.device_id,
                auth_token: server_token.clone(),
                server_url: local.server_url.clone(),
            };
            if let Err(e) = refreshed.save_to_file(&device_path) {
                warn!("保存刷新后的 device.yaml 失败: {}", e);
            }
            info!(
                "device {} token 已用 server 权威值刷新",
                refreshed.device_id
            );
            return Ok(refreshed);
        }
    }

    // 本地无记录 / 刚被删除 → 向 server 申报注册新 device
    info!("向 server 申报注册新 device");

    let resp = client
        .post(format!("{}/api/v1/device/register", http_url))
        .json(&serde_json::json!({}))
        .send()
        .await
        .context("调用 /device/register 失败")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("/device/register 失败: HTTP {} - {}", status, body);
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .context("Failed to parse /device/register response")?;
    let device_id = Uuid::parse_str(
        body["device_id"]
            .as_str()
            .context("/device/register 响应缺少 device_id")?,
    )
    .context("/device/register 返回的 device_id 不是合法 UUID")?;
    let auth_token = body["auth_token"]
        .as_str()
        .context("/device/register 响应缺少 auth_token")?
        .to_string();

    let device = DeviceConfig {
        device_id,
        auth_token,
        server_url: ws_url.to_string(),
    };

    if let Some(parent) = device_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    device.save_to_file(&device_path)?;

    info!(
        "新设备已向 server 申报注册: {} (server: {})",
        device_id, ws_url
    );
    Ok(device)
}

// ============================================================================
// 选择角色（从 filesystem）
// ============================================================================

pub(crate) fn select_character(server_dir: &Path) -> Option<CharacterConfig> {
    let chars_dir = server_dir.join("characters");
    if !chars_dir.exists() {
        return None;
    }

    let mut alive: Vec<CharacterConfig> = vec![];
    if let Ok(entries) = std::fs::read_dir(&chars_dir) {
        for entry in entries.flatten() {
            if !entry.file_type().ok()?.is_dir() {
                continue;
            }
            let path = entry.path().join("character.yaml");
            if let Ok(config) = CharacterConfig::from_file(&path)
                && config.status == CharacterStatus::Alive
            {
                alive.push(config);
            }
        }
    }

    alive.into_iter().next()
}

// ============================================================================
// 启动 Banner
// ============================================================================

/// 打印启动 Banner
pub(crate) fn print_startup_banner(
    port: u16,
    server_ws_url: &str,
    config_path_str: &str,
    mode: &str,
) {
    info!("╔══════════════════════════════════════════════╗");
    info!("║   Cyber-Jianghu Agent ({:^20})   ║", mode);
    info!("╠══════════════════════════════════════════════╣");
    info!("║ HTTP API:  http://0.0.0.0:{}                 ║", port);
    info!("║ WebSocket: {:<34} ║", server_ws_url);
    info!("║ Config:    {:<34} ║", config_path_str);
    info!("╠══════════════════════════════════════════════╣");
    info!("║ 切换服务器: POST /api/v1/config/server       ║");
    info!("║ 热加载配置: POST /api/v1/config/reload       ║");
    info!("║ API 文档:   GET  /api/v1                     ║");
    info!("╚══════════════════════════════════════════════╝");
}

// ============================================================================
// 日志系统初始化
// ============================================================================

pub(crate) fn init_tracing() -> Result<()> {
    let data_dir = cyber_jianghu_agent::config::data_base_dir();

    let thinking_log_path = thinking_log::init_thinking_log(&data_dir)?;

    // 训练 trace 结构化落盘（与 thinking_log 并列，init 之后）
    // 若 trace.yaml 缺失或 enabled=false，recorder 不初始化，零开销
    let config_dir = cyber_jianghu_agent::config::config_dir();
    cyber_jianghu_agent::infra::api::trace::init_trace_recorder(&config_dir);

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    info!(
        "日志系统已初始化，thinking log: {}",
        thinking_log_path.display()
    );

    Ok(())
}

// ============================================================================
// 主入口
// ============================================================================

// ============================================================================
// 命令实现
// ============================================================================
