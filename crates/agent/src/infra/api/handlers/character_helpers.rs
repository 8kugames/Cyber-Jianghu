// Helper Functions for Character Management
// ============================================================================

use std::path::Path;
use tracing::warn;
use uuid::Uuid;

use crate::config::{CharacterConfig, CharacterStatus};

use super::HttpApiState;
use anyhow::Context;

/// List all characters from filesystem
pub(crate) fn list_characters_from_fs(
    characters_dir: &Path,
) -> Result<Vec<CharacterConfig>, anyhow::Error> {
    if !characters_dir.exists() {
        return Ok(vec![]);
    }
    let mut chars = vec![];
    for entry in std::fs::read_dir(characters_dir).context("Failed to read characters dir")? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let char_yaml = entry.path().join("character.yaml");
        if !char_yaml.exists() {
            continue;
        }
        match CharacterConfig::from_file(&char_yaml) {
            Ok(c) => chars.push(c),
            Err(e) => warn!(
                "Skipping corrupted character.yaml in {:?}: {}",
                entry.path(),
                e
            ),
        }
    }
    Ok(chars)
}

/// Get active (alive) character from state
pub(crate) async fn get_active_character(
    state: &HttpApiState,
) -> Result<Option<CharacterConfig>, anyhow::Error> {
    let character_dir = state.character_dir.read().await.clone();
    let chars = list_characters_from_fs(&character_dir)?;
    Ok(chars
        .into_iter()
        .find(|c| c.status == CharacterStatus::Alive))
}

/// Get character config by agent_id (sync version for use in handlers)
pub(crate) fn get_character_by_id_sync(
    characters_dir: &std::path::Path,
    agent_id: Uuid,
) -> Result<Option<CharacterConfig>, anyhow::Error> {
    let chars = list_characters_from_fs(characters_dir)?;
    Ok(chars.into_iter().find(|c| c.agent_id == Some(agent_id)))
}

/// Save character config to its directory
pub(crate) fn save_character(
    config: &CharacterConfig,
    characters_dir: &Path,
) -> Result<(), anyhow::Error> {
    let agent_id = config
        .agent_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let dir = characters_dir.join(&agent_id);
    std::fs::create_dir_all(&dir)?;
    config.save_to_file(dir.join("character.yaml"))
}

/// Get device identity from state (async-safe)
pub(crate) async fn get_device_id(state: &HttpApiState) -> Result<(Uuid, String), anyhow::Error> {
    let device = state.device_config.read().await;
    let d = device.as_ref().context("No device identity")?;
    Ok((d.device_id, d.auth_token.clone()))
}

/// 位置层级表述：父链去掉区域级后与当前名以「·」连接（如 "龙门客栈·大堂"）。
///
/// 替代旧的 "名 (类型词)" 拼装：类型词（如 SubScene）对跨地图同名子场景无消歧力，
/// 层级链才能唯一定位。区域级不进表述（世界级上下文，顶栏保持紧凑）；
/// 无父链（地图/区域级位置）回落当前名。
pub(crate) fn hierarchical_location_name(location: &cyber_jianghu_protocol::Location) -> String {
    let chain = &location.parent_chain;
    if chain.len() > 1 {
        format!("{}·{}", chain[1..].join("·"), location.name)
    } else {
        location.name.clone()
    }
}

// ============================================================================
