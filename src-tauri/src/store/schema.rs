use rusqlite::Connection;

use crate::error::{AppError, AppResult};

/// M1-1 で確定するスキーマ版。以降のフェーズは +1 して match に 1 アーム足すだけでよい。
/// 2 = §20（M3-3: heuristics_enabled / silence_timeout_secs）
/// 3 = §29.1（M3-4: is_scratch）。この順序を入れ替えないこと（§34.1）
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

/// PRAGMA はトランザクションの外で適用済みであること（§46.3 の落とし穴 2）。
/// `&mut Connection` を取るのは Connection::transaction() が要求するため。
pub fn migrate(conn: &mut Connection) -> AppResult<()> {
    // 現行版を読むために、schema_version だけは版に依らず先に作る
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY NOT NULL);",
    )?;
    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?;

    // 1 版 = 1 トランザクション。途中でクラッシュしても、適用済みの版までは確定し、
    // 未適用の版は次回の open でやり直せる（§46.3 の落とし穴 1）
    for v in (current + 1)..=SCHEMA_VERSION {
        let tx = conn.transaction()?;
        match v {
            1 => tx.execute_batch(DDL_V1)?,
            // 2 => tx.execute_batch(MIGRATION_V2)?,   // §20（M3-3 が足す）
            // 3 => tx.execute_batch(MIGRATION_V3)?,   // §29.1（M3-4 が足す）
            other => return Err(AppError::Db(format!("unknown schema version {other}"))),
        }
        // 単一行を保つ。INSERT OR REPLACE は使えない（§46.3 の落とし穴 3）
        tx.execute("DELETE FROM schema_version", [])?;
        tx.execute("INSERT INTO schema_version (version) VALUES (?1)", [v])?;
        tx.commit()?;
    }
    Ok(())
}
