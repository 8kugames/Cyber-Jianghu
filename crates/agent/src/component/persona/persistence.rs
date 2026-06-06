use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use super::dynamic_persona::{DynamicPersona, PersonaState};
use super::trait_types::Trait;
use std::collections::HashMap;

/// Schema 版本:向前单调递增,新增列/表时 +1 并在 `apply_migration` 中追加分支
///
/// SQLite 的 `PRAGMA user_version` 是 schema 版本管理的官方模式(Django/Room 等均采用)。
/// persona_snapshot 表的 schema 演化路径:
/// - v1: 单行 UPSERT 模式,字段 agent_id/tick_id/traits_json/current_state_json/updated_at
const CURRENT_SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaPersistenceConfig {
    pub snapshot_interval_ticks: i64,
    pub flush_on_shutdown: bool,
    pub flush_on_death: bool,
}

impl Default for PersonaPersistenceConfig {
    fn default() -> Self {
        Self {
            snapshot_interval_ticks: 10,
            flush_on_shutdown: true,
            flush_on_death: true,
        }
    }
}

#[derive(Clone)]
pub struct PersonaStore {
    #[allow(dead_code)]
    agent_id: Uuid,
    conn: Arc<Mutex<Connection>>,
    #[allow(dead_code)]
    db_path: PathBuf,
    config: Arc<Mutex<PersonaPersistenceConfig>>,
}

impl PersonaStore {
    pub fn open(agent_id: Uuid, db_path: &Path, config: PersonaPersistenceConfig) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).context("创建 persona 持久化目录失败")?;
        }

        let conn = Connection::open(db_path).context("打开 persona 持久化数据库失败")?;
        init_pragmas(&conn)?;
        init_schema(&conn)?;

        Ok(Self {
            agent_id,
            conn: Arc::new(Mutex::new(conn)),
            db_path: db_path.to_path_buf(),
            config: Arc::new(Mutex::new(config)),
        })
    }

    pub fn load_or_default(&self, default: DynamicPersona) -> Result<DynamicPersona> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("persona conn lock poisoned: {}", e))?;

        let row = conn
            .query_row(
                "SELECT traits_json, current_state_json FROM persona_snapshot WHERE id = 1",
                [],
                |row| {
                    let traits_json: String = row.get(0)?;
                    let current_state_json: Option<String> = row.get(1)?;
                    Ok((traits_json, current_state_json))
                },
            )
            .optional()
            .context("查询 persona_snapshot 失败")?;

        let Some((traits_json, current_state_json)) = row else {
            return Ok(default);
        };

        let traits: HashMap<String, Trait> =
            serde_json::from_str(&traits_json).context("反序列化 persona traits_json 失败")?;

        let current_state = match current_state_json {
            Some(json) => serde_json::from_str::<PersonaState>(&json)
                .context("反序列化 persona current_state_json 失败")?,
            None => default.current_state.clone(),
        };

        Ok(DynamicPersona {
            agent_id: default.agent_id,
            name: default.name,
            base_description: default.base_description,
            traits,
            current_state,
            version: default.version,
        })
    }

    pub fn snapshot(&self, persona: &DynamicPersona, tick_id: i64) -> Result<()> {
        self.write_snapshot(persona, tick_id)
    }

    pub fn snapshot_now(&self, persona: &DynamicPersona, tick_id: i64) -> Result<()> {
        self.write_snapshot(persona, tick_id)
    }

    fn write_snapshot(&self, persona: &DynamicPersona, tick_id: i64) -> Result<()> {
        let traits_json =
            serde_json::to_string(&persona.traits).context("序列化 persona traits 失败")?;
        let current_state_json = serde_json::to_string(&persona.current_state)
            .context("序列化 persona current_state 失败")?;

        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("persona conn lock poisoned: {}", e))?;

        conn.execute(
            "INSERT INTO persona_snapshot (id, agent_id, tick_id, traits_json, current_state_json, updated_at)
             VALUES (1, ?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)
             ON CONFLICT(id) DO UPDATE SET
                agent_id = excluded.agent_id,
                tick_id = excluded.tick_id,
                traits_json = excluded.traits_json,
                current_state_json = excluded.current_state_json,
                updated_at = CURRENT_TIMESTAMP",
            params![
                self.agent_id.to_string(),
                tick_id,
                traits_json,
                current_state_json,
            ],
        )
        .context("写入 persona_snapshot 失败")?;

        Ok(())
    }

    pub fn update_config(&self, new_config: PersonaPersistenceConfig) -> Result<()> {
        let mut guard = self
            .config
            .lock()
            .map_err(|e| anyhow::anyhow!("persona config lock poisoned: {}", e))?;
        *guard = new_config;
        Ok(())
    }

    pub fn config_snapshot_interval(&self) -> i64 {
        self.config
            .lock()
            .map(|c| c.snapshot_interval_ticks)
            .unwrap_or(10)
    }

    pub fn config_flush_on_shutdown(&self) -> bool {
        self.config
            .lock()
            .map(|c| c.flush_on_shutdown)
            .unwrap_or(true)
    }

    pub fn config_flush_on_death(&self) -> bool {
        self.config.lock().map(|c| c.flush_on_death).unwrap_or(true)
    }
}

fn init_pragmas(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")
        .context("设置 persona SQLite PRAGMA 失败")?;
    Ok(())
}

fn init_schema(conn: &Connection) -> Result<()> {
    let current_version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .context("读取 PRAGMA user_version 失败")?;

    for version in (current_version + 1)..=CURRENT_SCHEMA_VERSION {
        apply_migration(conn, version)
            .with_context(|| format!("应用 schema 迁移 v{} 失败", version))?;
        set_user_version(conn, version)
            .with_context(|| format!("设置 PRAGMA user_version = {} 失败", version))?;
    }

    Ok(())
}

fn apply_migration(conn: &Connection, version: i64) -> Result<()> {
    match version {
        1 => conn
            .execute(
                "CREATE TABLE IF NOT EXISTS persona_snapshot (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    agent_id TEXT NOT NULL,
                    tick_id INTEGER NOT NULL,
                    traits_json TEXT NOT NULL,
                    current_state_json TEXT,
                    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
                )",
                [],
            )
            .context("创建 persona_snapshot 表失败")
            .map(|_| ())?,
        _ => bail!("未知 schema 版本: {}", version),
    }
    Ok(())
}

fn set_user_version(conn: &Connection, version: i64) -> Result<()> {
    conn.execute_batch(&format!("PRAGMA user_version = {};", version))
        .context("设置 PRAGMA user_version 失败")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_schema_sets_user_version_to_current_on_fresh_db() {
        let conn = Connection::open_in_memory().unwrap();
        init_pragmas(&conn).unwrap();
        init_schema(&conn).unwrap();

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn init_schema_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        init_pragmas(&conn).unwrap();
        init_schema(&conn).unwrap();
        init_schema(&conn).unwrap();

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn open_advances_user_version_when_db_is_stale() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = dir.path().join("persona.db");
        let agent_id = Uuid::new_v4();

        // 模拟老版本 DB:PRAGMA user_version = 0
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch("PRAGMA user_version = 0;").unwrap();
        }

        // 打开后应自动迁移到 CURRENT_SCHEMA_VERSION
        let _store =
            PersonaStore::open(agent_id, &db, PersonaPersistenceConfig::default()).unwrap();

        let conn = Connection::open(&db).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }
}
