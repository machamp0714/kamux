use rusqlite::Connection;

use crate::error::{AppError, AppResult};

/// M1-1 で確定するスキーマ版。以降のフェーズは +1 して match に 1 アーム足すだけでよい。
/// 2 = §20（M3-3: heuristics_enabled / silence_timeout_secs）
/// 3 = §29.1（M3-4: is_scratch）。この順序を入れ替えないこと（§34.1）
pub const SCHEMA_VERSION: i64 = 2;

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

/// 契約 §20 の DDL をそのまま写したもの（M3-3: 汎用 CLI ヒューリスティックの
/// セッション単位設定）。
///
/// **`DDL_V1` 側にこの 2 列を足してはならない。** `migrate` は新規 DB でも
/// `current = 0` から v1 → v2 を順に適用するため、`DDL_V1` が列を持っていると
/// 直後の `ALTER TABLE ADD COLUMN` が `duplicate column name` で必ず失敗し、
/// 新規 DB が 1 つも開けなくなる。
///
/// 既存行は一律 `DEFAULT 1` / `DEFAULT 30` で埋まる。`cli_kind` ごとの
/// 既定値（`default_heuristics_enabled`）は新規セッションの構築点
/// （`Session::new_backlog`）が持つ責務であり、ここでは分岐しない。
const MIGRATION_V2: &str = r#"
ALTER TABLE sessions ADD COLUMN heuristics_enabled   INTEGER NOT NULL DEFAULT 1;
ALTER TABLE sessions ADD COLUMN silence_timeout_secs INTEGER NOT NULL DEFAULT 30;
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
            2 => tx.execute_batch(MIGRATION_V2)?, // §20（M3-3）
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

#[cfg(test)]
mod migration_v2_tests {
    use super::*;

    /// 実物の `DDL_V1` で v1 の DB を作る。v1 スキーマを手書きしない ——
    /// 手書きすると v1 の定義がずれて、測っているものが本物のアップグレード
    /// 経路でなくなる。`schema_version` に 1 を記録することで、`migrate` が
    /// `(1+1)..=SCHEMA_VERSION` すなわち v2 だけを適用する経路に入る。
    fn v1_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(DDL_V1).expect("v1 ddl");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY NOT NULL);
             INSERT INTO schema_version (version) VALUES (1);",
        )
        .expect("record version 1");
        conn
    }

    fn columns(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("PRAGMA table_info(sessions)")
            .expect("prepare pragma");
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .expect("query pragma");
        rows.map(|r| r.expect("column name")).collect()
    }

    /// v1 の時代に既に存在していた行を 1 本作る（バックフィルの観測対象）。
    fn insert_v1_session(conn: &Connection, id: &str, cli_kind: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO projects (id, name, repo_path, created_at, updated_at)
             VALUES ('p1', 'kamux', '/Users/x/repo/kamux', 0, 0)",
            [],
        )
        .expect("insert project");
        conn.execute(
            "INSERT INTO sessions (id, project_id, title, sort_order, mode, cli_kind,
                                   created_at, updated_at)
             VALUES (?1, 'p1', 'old row', 1.0, 'in_place', ?2, 0, 0)",
            rusqlite::params![id, cli_kind],
        )
        .expect("insert v1 session");
    }

    #[test]
    fn migrate_adds_the_two_heuristic_columns_to_a_v1_database() {
        let mut conn = v1_conn();
        migrate(&mut conn).expect("migrate");

        let cols = columns(&conn);
        assert!(
            cols.contains(&"heuristics_enabled".to_owned()),
            "heuristics_enabled が追加されていない: {cols:?}"
        );
        assert!(
            cols.contains(&"silence_timeout_secs".to_owned()),
            "silence_timeout_secs が追加されていない: {cols:?}"
        );
    }

    #[test]
    fn migrate_backfills_existing_v1_rows_with_the_contract_defaults() {
        // 契約 §20 の DDL は既存行を一律 `DEFAULT 1` / `DEFAULT 30` で埋める。
        // `insert_session` は値を明示バインドするので、DDL の DEFAULT が観測される
        // 経路はこのバックフィルだけである。
        //
        // 副作用（意図どおり）: 移行前から存在する cli_kind = 'shell' の行も
        // heuristics_enabled = 1 になる。`default_heuristics_enabled(Shell) == false`
        // と食い違うが、契約 §20 の SQL が正典なので UPDATE で直さない。
        let mut conn = v1_conn();
        insert_v1_session(&conn, "s1", "claude");
        insert_v1_session(&conn, "s2", "shell");

        migrate(&mut conn).expect("migrate");

        let (enabled, secs): (i64, i64) = conn
            .query_row(
                "SELECT heuristics_enabled, silence_timeout_secs FROM sessions WHERE id = 's1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("read back s1");
        assert_eq!(enabled, 1, "heuristics_enabled の既定は 1");
        assert_eq!(secs, 30, "silence_timeout_secs の既定は 30");

        let shell_enabled: i64 = conn
            .query_row(
                "SELECT heuristics_enabled FROM sessions WHERE id = 's2'",
                [],
                |r| r.get(0),
            )
            .expect("read back s2");
        assert_eq!(
            shell_enabled, 1,
            "既存の shell 行も一律 1 になる（契約 §20 の DDL どおり）"
        );
    }

    #[test]
    fn migrate_records_version_2() {
        let mut conn = v1_conn();
        migrate(&mut conn).expect("migrate");

        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .expect("version");
        assert_eq!(version, 2);
        assert_eq!(SCHEMA_VERSION, 2, "契約 §20: M3-3 の版は 2");
    }

    #[test]
    fn migrate_keeps_schema_version_as_a_single_row_after_applying_v2() {
        // §46.3 の落とし穴 3。`INSERT OR REPLACE` / `INSERT OR IGNORE` で 2 を書くと
        // version が PRIMARY KEY のため衝突せず、単に 2 行目が増える。
        // `MAX(version) == 2` だけを見るテストは 2 行あっても緑になるので、
        // 行数を独立に固定する。
        let mut conn = v1_conn();
        migrate(&mut conn).expect("migrate");

        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
            .expect("count");
        assert_eq!(rows, 1, "v2 適用後に version 行が増えている");
    }

    #[test]
    fn migrating_an_already_migrated_v2_database_is_a_no_op() {
        // `ALTER TABLE ADD COLUMN` に IF NOT EXISTS 形は無い（§46.3 の落とし穴 1）。
        // 2 度目の migrate が v2 を再実行すると duplicate column name で失敗する。
        let mut conn = v1_conn();
        migrate(&mut conn).expect("first migrate");
        migrate(&mut conn).expect("2 度目の migrate が失敗した");

        assert_eq!(
            columns(&conn)
                .iter()
                .filter(|c| *c == "heuristics_enabled")
                .count(),
            1
        );
    }
}
