use anyhow::{Context, Result, bail};
use rusqlite::Connection;

const CURRENT_SCHEMA_VERSION: i64 = 1;

pub(super) fn init_pragmas(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA cache_size = -31744;",
    )
    .context("设置 relationship SQLite PRAGMA 失败")?;
    Ok(())
}

pub(super) fn init_schema(conn: &Connection) -> Result<()> {
    let current_version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .context("读取 PRAGMA user_version 失败")?;

    for version in (current_version + 1)..=CURRENT_SCHEMA_VERSION {
        apply_migration(conn, version)
            .with_context(|| format!("应用 relationship schema 迁移 v{} 失败", version))?;
        set_user_version(conn, version)
            .with_context(|| format!("设置 PRAGMA user_version = {} 失败", version))?;
    }

    Ok(())
}

fn apply_migration(conn: &Connection, version: i64) -> Result<()> {
    match version {
        1 => {
            conn.execute(
                "CREATE TABLE IF NOT EXISTS relationships (
                    target_agent_id TEXT PRIMARY KEY,
                    target_name TEXT NOT NULL,
                    favorability INTEGER DEFAULT 0 CHECK(favorability >= -100 AND favorability <= 100),
                    last_interaction_tick INTEGER DEFAULT 0,
                    updated_at TIMESTAMP NOT NULL,
                    self_description TEXT DEFAULT '',
                    description_tick INTEGER DEFAULT 0
                )",
                [],
            )
            .context("创建 relationships 表失败")?;

            let has_self_description: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('relationships') WHERE name = 'self_description'",
                    [],
                    |row| row.get::<_, i32>(0).map(|c| c > 0),
                )
                .unwrap_or(false);
            if !has_self_description {
                conn.execute(
                    "ALTER TABLE relationships ADD COLUMN self_description TEXT DEFAULT ''",
                    [],
                )
                .context("ALTER TABLE 添加 self_description 列失败")?;
            }

            let has_description_tick: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('relationships') WHERE name = 'description_tick'",
                    [],
                    |row| row.get::<_, i32>(0).map(|c| c > 0),
                )
                .unwrap_or(false);
            if !has_description_tick {
                conn.execute(
                    "ALTER TABLE relationships ADD COLUMN description_tick INTEGER DEFAULT 0",
                    [],
                )
                .context("ALTER TABLE 添加 description_tick 列失败")?;
            }

            conn.execute(
                "CREATE TABLE IF NOT EXISTS key_events (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    target_agent_id TEXT NOT NULL,
                    tick_id INTEGER NOT NULL,
                    event_type TEXT NOT NULL,
                    description TEXT NOT NULL,
                    favorability_delta INTEGER NOT NULL,
                    timestamp TEXT NOT NULL,
                    FOREIGN KEY (target_agent_id) REFERENCES relationships(target_agent_id) ON DELETE CASCADE
                )",
                [],
            )
            .context("创建 key_events 表失败")?;

            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_key_events_target
                 ON key_events(target_agent_id)",
                [],
            )
            .context("创建 idx_key_events_target 失败")?;

            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_relationships_target_name
                 ON relationships(target_name)",
                [],
            )
            .context("创建 idx_relationships_target_name 失败")?;

            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_key_events_tick
                 ON key_events(tick_id DESC)",
                [],
            )
            .context("创建 idx_key_events_tick 失败")?;

            Ok(())
        }
        _ => bail!("未知 relationship schema 版本: {}", version),
    }
}

fn set_user_version(conn: &Connection, version: i64) -> Result<()> {
    conn.execute_batch(&format!("PRAGMA user_version = {};", version))
        .context("设置 relationship PRAGMA user_version 失败")?;
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
    fn init_schema_adds_missing_columns_to_legacy_5col_db() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE relationships (
                target_agent_id TEXT PRIMARY KEY,
                target_name TEXT NOT NULL,
                favorability INTEGER DEFAULT 0,
                last_interaction_tick INTEGER DEFAULT 0,
                updated_at TIMESTAMP NOT NULL
            );",
        )
        .unwrap();

        init_pragmas(&conn).unwrap();
        init_schema(&conn).unwrap();

        let has_self_description: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('relationships') WHERE name = 'self_description'",
                [],
                |row| row.get::<_, i32>(0).map(|c| c > 0),
            )
            .unwrap();
        let has_description_tick: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('relationships') WHERE name = 'description_tick'",
                [],
                |row| row.get::<_, i32>(0).map(|c| c > 0),
            )
            .unwrap();

        assert!(has_self_description, "self_description 列必须存在");
        assert!(has_description_tick, "description_tick 列必须存在");
    }
}
