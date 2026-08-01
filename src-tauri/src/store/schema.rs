use rusqlite::Connection;

use crate::error::AppResult;

/// M1-1 で確定するスキーマ版。以降のフェーズでスキーマを変えるときは
/// この値を +1 し、migrate() に差分 SQL の分岐を追加する。
/// 次の変更は契約 §20（M3-3 が version 2 で heuristics_enabled /
/// silence_timeout_secs を ALTER TABLE で追加）。
pub const SCHEMA_VERSION: i64 = 1;

const DDL_V1: &str = r#"
CREATE TABLE IF NOT EXISTS projects (
    id           TEXT    PRIMARY KEY NOT NULL,
    name         TEXT    NOT NULL,
    repo_path    TEXT    NOT NULL UNIQUE,
    default_cli  TEXT    NOT NULL DEFAULT 'claude',
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    id                 TEXT    PRIMARY KEY NOT NULL,
    project_id         TEXT    NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    title              TEXT    NOT NULL,
    description        TEXT    NOT NULL DEFAULT '',
    kanban_status      TEXT    NOT NULL DEFAULT 'backlog',
    sort_order         REAL    NOT NULL,
    mode               TEXT    NOT NULL,
    branch             TEXT,
    worktree_path      TEXT,
    cli_kind           TEXT    NOT NULL,
    cli_command        TEXT,
    claude_session_id  TEXT,
    last_runtime_state TEXT    NOT NULL DEFAULT 'idle',
    last_runtime_error TEXT,
    first_started_at   INTEGER,
    archived_at        INTEGER,
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sessions_project_status
    ON sessions (project_id, kanban_status, sort_order);
"#;

/// PRAGMA は接続ごとの設定。foreign_keys は特に永続化されないので
/// 新しい Connection を開くたびに必ず適用する。
/// journal_mode = WAL は結果行を返すため execute() ではなく execute_batch() を使う。
pub fn apply_pragmas(conn: &Connection) -> AppResult<()> {
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
    Ok(())
}

pub fn migrate(conn: &Connection) -> AppResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY NOT NULL);",
    )?;
    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?;

    if current < 1 {
        conn.execute_batch(DDL_V1)?;
        conn.execute("INSERT INTO schema_version (version) VALUES (?1)", [1i64])?;
    }

    // 次の版はここに `if current < 2 { ... }` を足す（契約 §20 / M3-3）。
    // 分岐は累積適用なので、古い DB でも 1 -> 2 と順に流れる。

    Ok(())
}
