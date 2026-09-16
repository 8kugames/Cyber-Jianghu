//! CLI 子命令实现（config/character/reset）

use super::*;

pub(crate) fn show_config() -> Result<()> {
    let config = load_config()?.ok_or_else(|| {
        anyhow::anyhow!("配置文件不存在（character_generation 为必填项，无法使用默认配置）")
    })?;

    println!("=== Agent 配置 ===\n");

    println!("服务器配置:");
    println!("  WebSocket: {}", config.server.ws_url);
    println!("  HTTP: {}", config.server.http_url);

    // Show device status for default server
    let server_dir = config.server_dir(&config.server.ws_url);
    let device_path = config.device_yaml_path(&config.server.ws_url);
    if device_path.exists() {
        if let Ok(device) = DeviceConfig::from_file(&device_path) {
            println!("\n设备身份:");
            println!("  Device ID: {}", device.device_id);
            println!(
                "  Auth Token: {}...",
                device.auth_token.chars().take(16).collect::<String>()
            );
        }
    } else {
        println!("\n设备身份: (未注册)");
    }

    // Show characters for this server
    if let Some(character) = select_character(&server_dir) {
        println!("\n当前角色:");
        println!("  姓名: {}", character.name);
        println!("  年龄: {}", character.age);
        println!("  性别: {}", character.gender);
        if let Some(ref agent_id) = character.agent_id {
            println!("  Agent ID: {}", agent_id);
        } else {
            println!("  Agent ID: (未注册)");
        }
    } else {
        println!("\n当前角色: (未创建)");
        let display_port = if config.runtime.port == 0 {
            "<自动>".to_string()
        } else {
            config.runtime.port.to_string()
        };
        println!("  通过 Web 面板创建: http://localhost:{}/", display_port);
        println!("  或通过 CLI: cyber-jianghu-agent create-character --name 名字");
    }

    println!("\n运行时配置:");
    println!("  模式: {:?}", config.runtime.mode);
    println!("  端口: {}", config.runtime.port);

    Ok(())
}

pub(crate) fn update_server_config(ws_url: Option<String>, http_url: Option<String>) -> Result<()> {
    let mut config = load_config()?.ok_or_else(|| {
        anyhow::anyhow!("配置文件不存在（character_generation 为必填项，无法使用默认配置）")
    })?;

    if let Some(ws) = ws_url {
        config.server.ws_url = ws;
    }
    if let Some(http) = http_url {
        config.server.http_url = http;
    }

    save_config(&config)?;
    info!("服务器配置已更新");
    Ok(())
}

pub(crate) async fn create_character_cli(
    name: String,
    age: u8,
    gender: String,
    appearance: Option<String>,
    identity: Option<String>,
) -> Result<()> {
    // 检查 Agent 是否已运行
    let port = 23340; // 默认端口

    let character = CharacterConfig {
        name,
        age,
        gender,
        appearance,
        identity,
        ..Default::default()
    };

    match create_character_via_api(port, character).await {
        Ok(agent_id) => {
            info!("角色创建成功! Agent ID: {}", agent_id);
            println!("角色已创建，Agent ID: {}", agent_id);
        }
        Err(e) => {
            warn!("无法连接到 Agent API: {}", e);
            warn!("请确保 Agent 已启动并监听端口 {}", port);
            warn!("或通过 Web 面板创建角色: http://localhost:{}/", port);
            return Err(e);
        }
    }

    Ok(())
}

pub(crate) fn reset_agent() -> Result<()> {
    let path = config_path();

    warn!("即将删除配置文件: {}", path.display());
    println!("确认删除? (y/N): ");

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    if input.trim().to_lowercase() == "y" {
        if path.exists() {
            std::fs::remove_file(&path)?;
            info!("配置文件已删除");
        }
        println!("Agent 身份已重置，下次启动将生成新的身份");
    } else {
        println!("已取消");
    }

    Ok(())
}

// ============================================================================
// LLM 客户端工厂（Claw vs Cognitive 的唯一架构差异）
// ============================================================================
