use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::error::{AppError, AppResult};

/// 契約 §0: DB は ~/Library/Application Support/kamux/app.db。
/// Tauri の app_data_dir() はバンドル identifier を含むパスを返すため使わない。
/// テストと将来の CI が本番 DB を汚さないよう、KAMUX_DB_PATH で上書きできる。
pub fn db_path() -> AppResult<PathBuf> {
    if let Some(overridden) = std::env::var_os("KAMUX_DB_PATH") {
        return Ok(PathBuf::from(overridden));
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| AppError::Io("HOME environment variable is not set".to_owned()))?;
    Ok(PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("kamux")
        .join("app.db"))
}

/// 契約 §3: 時刻は Unix epoch ミリ秒の INTEGER。
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub mod project_dao;
pub mod schema;
pub mod session_dao;

/// SQLite 接続の唯一の持ち主。全 DAO メソッドは同期で、
/// MutexGuard を .await を跨いで保持しない（Tauri の async コマンドが Send を要求するため）。
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> AppResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut conn = Connection::open(path)?;
        schema::apply_pragmas(&conn)?;
        schema::migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub(crate) fn conn(&self) -> AppResult<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| AppError::Db("store mutex is poisoned".to_owned()))
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::{now_ms, Store};
    use crate::model::{CliKind, KanbanStatus, Session, SessionMode};
    use tempfile::TempDir;

    /// テンポラリディレクトリ上に初期化済みの Store を作る。
    /// TempDir を返すのは、束縛を落とすとディレクトリごと消えるため。
    pub(crate) fn open_temp() -> (TempDir, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(&dir.path().join("app.db")).expect("open store");
        (dir, store)
    }

    /// 「採番 → 構築 → 挿入」の 3 手をまとめたテスト用ヘルパ。
    /// in_place / shell / branch なしの最小構成。worktree や cli_command を
    /// 検証したいテストは Session::new_backlog を直接使うこと。
    pub(crate) fn insert_test_session(store: &Store, project_id: &str, title: &str) -> Session {
        let sort_order = store
            .next_sort_order(project_id, KanbanStatus::Backlog)
            .expect("next_sort_order");
        let session = Session::new_backlog(
            project_id,
            title,
            "",
            SessionMode::InPlace,
            None,
            CliKind::Shell,
            None,
            sort_order,
            now_ms(),
        );
        store.insert_session(&session).expect("insert_session")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 環境変数はプロセス全体で共有されるため、KAMUX_DB_PATH を触るテストは
    // このひとつだけに閉じ込める（他のテストは Store::open に明示パスを渡す）。
    #[test]
    fn db_path_honors_override_then_falls_back_to_application_support() {
        std::env::set_var("KAMUX_DB_PATH", "/tmp/kamux-override.db");
        assert_eq!(
            db_path().expect("path"),
            PathBuf::from("/tmp/kamux-override.db")
        );

        std::env::remove_var("KAMUX_DB_PATH");
        let home = std::env::var("HOME").expect("HOME");
        assert_eq!(
            db_path().expect("path"),
            PathBuf::from(home).join("Library/Application Support/kamux/app.db")
        );
    }

    #[test]
    fn now_ms_returns_a_plausible_epoch_millisecond() {
        let now = now_ms();
        // 2020-01-01T00:00:00Z より後で、ミリ秒スケールであること
        assert!(now > 1_577_836_800_000, "epoch ミリ秒になっていない: {now}");
        assert!(now < 4_102_444_800_000, "秒とミリ秒を取り違えている: {now}");
    }

    #[test]
    fn now_ms_is_monotonic_enough_for_updated_at() {
        let a = now_ms();
        std::thread::sleep(std::time::Duration::from_millis(3));
        assert!(now_ms() > a);
    }

    #[test]
    fn open_creates_parent_directories_and_schema() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("app.db");
        let store = Store::open(&path).expect("open");
        assert!(path.exists(), "DB ファイルが作られていない");

        let conn = store.conn().expect("conn");
        let tables: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
                .expect("prepare");
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .expect("query");
            rows.map(|r| r.expect("row")).collect()
        };
        assert!(tables.contains(&"projects".to_owned()));
        assert!(tables.contains(&"sessions".to_owned()));
        assert!(tables.contains(&"schema_version".to_owned()));

        let index: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_sessions_project_status'",
                [],
                |r| r.get(0),
            )
            .expect("index");
        assert_eq!(index, 1);
    }

    #[test]
    fn open_applies_wal_and_foreign_keys_pragmas() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(&dir.path().join("app.db")).expect("open");
        let conn = store.conn().expect("conn");

        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .expect("mode");
        assert_eq!(mode, "wal");

        // foreign_keys は接続ごとの設定で永続化されない。open で必ず入ること。
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .expect("fk");
        assert_eq!(fk, 1);
    }

    #[test]
    fn open_records_schema_version_1_and_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("app.db");

        {
            let store = Store::open(&path).expect("first open");
            let conn = store.conn().expect("conn");
            let version: i64 = conn
                .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
                .expect("version");
            assert_eq!(version, schema::SCHEMA_VERSION);
        }

        let store = Store::open(&path).expect("second open");
        let conn = store.conn().expect("conn");
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
            .expect("count");
        assert_eq!(rows, 1, "再オープンでバージョン行が増えてはいけない");
    }

    #[test]
    fn foreign_keys_pragma_actually_rejects_orphan_sessions() {
        let (_dir, store) = test_support::open_temp();
        let conn = store.conn().expect("conn");
        let result = conn.execute(
            "INSERT INTO sessions (id, project_id, title, sort_order, mode, cli_kind, created_at, updated_at)
             VALUES ('s1', 'missing-project', 't', 1.0, 'in_place', 'shell', 1, 1)",
            [],
        );
        assert!(result.is_err(), "存在しない project_id を弾いていない");
    }

    // COALESCE(MAX(version), 0) は「空テーブル」と「version = 0 の行が 1 本ある」を
    // 区別できない。version 1 しか存在しない現状では 2 回目の open のループ本体が
    // 空範囲になり素通りしてしまうので、意図的に「version = 0 の迷子行」を作って
    // ループ本体（DELETE → INSERT）を強制的に再実行させる（契約 §46.3 落とし穴 3）。
    #[test]
    fn migrate_keeps_schema_version_as_a_single_row_after_reapplication() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("app.db");

        {
            let store = Store::open(&path).expect("first open");
            let conn = store.conn().expect("conn");
            conn.execute("DELETE FROM schema_version", [])
                .expect("clear schema_version");
            conn.execute("INSERT INTO schema_version (version) VALUES (0)", [])
                .expect("seed a stray version 0 row");
        }

        let store = Store::open(&path).expect("second open re-runs migrate for version 1");
        let conn = store.conn().expect("conn");
        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .expect("version");
        assert_eq!(version, schema::SCHEMA_VERSION);

        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
            .expect("count");
        assert_eq!(
            rows, 1,
            "DELETE を伴わない更新（INSERT OR REPLACE 相当）だと version = 0 の迷子行が残る"
        );
    }

    // 期待値は契約 §3（行 218-266）の DDL からそのまま書き写す。実装の DDL_V1 は
    // 参照しない —— 実装から期待値を作ると、実装が契約からずれても一緒にずれて
    // 検出できなくなる（契約 §37.2 の last_runtime_error 事故の再発防止）。
    #[test]
    fn projects_table_matches_contract_ddl_columns_notnull_and_defaults() {
        use crate::model::CliKind;

        let (_dir, store) = test_support::open_temp();
        let conn = store.conn().expect("conn");

        let mut stmt = conn
            .prepare("PRAGMA table_info(projects)")
            .expect("prepare");
        let rows: Vec<(i64, String, String, i64, Option<String>, i64)> = stmt
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })
            .expect("query")
            .map(|r| r.expect("row"))
            .collect();

        // dflt_value は SQLite が文字列リテラルをクォート込みで返す（実測で確認済み）。
        let claude = format!("'{}'", CliKind::Claude.as_db_str());
        let expected = vec![
            (0, "id".to_owned(), "TEXT".to_owned(), 1, None, 1),
            (1, "name".to_owned(), "TEXT".to_owned(), 1, None, 0),
            (2, "repo_path".to_owned(), "TEXT".to_owned(), 1, None, 0),
            (
                3,
                "default_cli".to_owned(),
                "TEXT".to_owned(),
                1,
                Some(claude),
                0,
            ),
            (4, "created_at".to_owned(), "INTEGER".to_owned(), 1, None, 0),
            (5, "updated_at".to_owned(), "INTEGER".to_owned(), 1, None, 0),
        ];

        assert_eq!(rows, expected);
    }

    // 期待値は契約 §34.2（行 2444-2465）の DDL からそのまま書き写す（理由は上のテストと同じ）。
    #[test]
    fn sessions_table_matches_contract_ddl_columns_notnull_and_defaults() {
        use crate::model::{KanbanStatus, RuntimeState};

        let (_dir, store) = test_support::open_temp();
        let conn = store.conn().expect("conn");

        let mut stmt = conn
            .prepare("PRAGMA table_info(sessions)")
            .expect("prepare");
        let rows: Vec<(i64, String, String, i64, Option<String>, i64)> = stmt
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })
            .expect("query")
            .map(|r| r.expect("row"))
            .collect();

        let backlog = format!("'{}'", KanbanStatus::Backlog.as_db_str());
        let idle = format!("'{}'", RuntimeState::Idle.as_db_str());
        let expected = vec![
            (0, "id".to_owned(), "TEXT".to_owned(), 1, None, 1),
            (1, "project_id".to_owned(), "TEXT".to_owned(), 1, None, 0),
            (2, "title".to_owned(), "TEXT".to_owned(), 1, None, 0),
            (
                3,
                "description".to_owned(),
                "TEXT".to_owned(),
                1,
                Some("''".to_owned()),
                0,
            ),
            (
                4,
                "kanban_status".to_owned(),
                "TEXT".to_owned(),
                1,
                Some(backlog),
                0,
            ),
            (5, "sort_order".to_owned(), "REAL".to_owned(), 1, None, 0),
            (6, "mode".to_owned(), "TEXT".to_owned(), 1, None, 0),
            (7, "branch".to_owned(), "TEXT".to_owned(), 0, None, 0),
            (8, "worktree_path".to_owned(), "TEXT".to_owned(), 0, None, 0),
            (9, "cli_kind".to_owned(), "TEXT".to_owned(), 1, None, 0),
            (10, "cli_command".to_owned(), "TEXT".to_owned(), 0, None, 0),
            (
                11,
                "claude_session_id".to_owned(),
                "TEXT".to_owned(),
                0,
                None,
                0,
            ),
            (
                12,
                "last_runtime_state".to_owned(),
                "TEXT".to_owned(),
                1,
                Some(idle),
                0,
            ),
            (
                13,
                "last_runtime_error".to_owned(),
                "TEXT".to_owned(),
                0,
                None,
                0,
            ),
            (
                14,
                "first_started_at".to_owned(),
                "INTEGER".to_owned(),
                0,
                None,
                0,
            ),
            (
                15,
                "archived_at".to_owned(),
                "INTEGER".to_owned(),
                0,
                None,
                0,
            ),
            (
                16,
                "created_at".to_owned(),
                "INTEGER".to_owned(),
                1,
                None,
                0,
            ),
            (
                17,
                "updated_at".to_owned(),
                "INTEGER".to_owned(),
                1,
                None,
                0,
            ),
        ];

        assert_eq!(rows, expected);
    }
}
