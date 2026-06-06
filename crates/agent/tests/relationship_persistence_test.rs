use rusqlite::Connection;
use tempfile::TempDir;
use uuid::Uuid;

use cyber_jianghu_agent::component::social::RelationshipStore;

fn read_user_version(db_path: &std::path::Path) -> i64 {
    let conn = Connection::open(db_path).unwrap();
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    let count: i32 = conn
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name = '{}'",
                table, column
            ),
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    count > 0
}

fn index_exists(conn: &Connection, name: &str) -> bool {
    let count: i32 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?",
            [name],
            |row| row.get(0),
        )
        .unwrap_or(0);
    count > 0
}

#[test]
fn open_fresh_db_advances_user_version_to_one() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("rel.db");

    let _store = RelationshipStore::open(Uuid::new_v4(), &db).unwrap();

    assert_eq!(read_user_version(&db), 1);
}

#[test]
fn open_is_idempotent_across_multiple_calls() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("rel.db");
    let agent_id = Uuid::new_v4();

    let _s1 = RelationshipStore::open(agent_id, &db).unwrap();
    let _s2 = RelationshipStore::open(agent_id, &db).unwrap();
    let _s3 = RelationshipStore::open(agent_id, &db).unwrap();

    assert_eq!(read_user_version(&db), 1);
}

#[test]
fn open_migrates_legacy_db_without_user_version_to_v1() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("rel.db");

    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE relationships (
                target_agent_id TEXT PRIMARY KEY,
                target_name TEXT NOT NULL,
                favorability INTEGER DEFAULT 0,
                last_interaction_tick INTEGER DEFAULT 0,
                updated_at TIMESTAMP NOT NULL
            );
            CREATE TABLE key_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                target_agent_id TEXT NOT NULL,
                tick_id INTEGER NOT NULL,
                event_type TEXT NOT NULL,
                description TEXT NOT NULL,
                favorability_delta INTEGER NOT NULL,
                timestamp TEXT NOT NULL
            );
            PRAGMA user_version = 0;",
        )
        .unwrap();
    }

    let _store = RelationshipStore::open(Uuid::new_v4(), &db).unwrap();

    assert_eq!(read_user_version(&db), 1);

    let conn = Connection::open(&db).unwrap();
    assert!(column_exists(&conn, "relationships", "self_description"));
    assert!(column_exists(&conn, "relationships", "description_tick"));
}

#[test]
fn open_creates_all_v1_columns_on_fresh_db() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("rel.db");

    let _store = RelationshipStore::open(Uuid::new_v4(), &db).unwrap();

    let conn = Connection::open(&db).unwrap();
    for col in [
        "target_agent_id",
        "target_name",
        "favorability",
        "last_interaction_tick",
        "updated_at",
        "self_description",
        "description_tick",
    ] {
        assert!(
            column_exists(&conn, "relationships", col),
            "relationships.{} 必须存在",
            col
        );
    }
    for col in [
        "target_agent_id",
        "tick_id",
        "event_type",
        "description",
        "favorability_delta",
        "timestamp",
    ] {
        assert!(
            column_exists(&conn, "key_events", col),
            "key_events.{} 必须存在",
            col
        );
    }
}

#[test]
fn open_creates_all_v1_indexes_on_fresh_db() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("rel.db");

    let _store = RelationshipStore::open(Uuid::new_v4(), &db).unwrap();

    let conn = Connection::open(&db).unwrap();
    for idx in [
        "idx_key_events_target",
        "idx_relationships_target_name",
        "idx_key_events_tick",
    ] {
        assert!(index_exists(&conn, idx), "{} 索引必须存在", idx);
    }
}
