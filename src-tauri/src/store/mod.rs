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

pub mod schema;

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
        let conn = Connection::open(path)?;
        schema::apply_pragmas(&conn)?;
        schema::migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn conn(&self) -> AppResult<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| AppError::Db("store mutex is poisoned".to_owned()))
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::Store;
    use tempfile::TempDir;

    /// テンポラリディレクトリ上に初期化済みの Store を作る。
    /// TempDir を返すのは、束縛を落とすとディレクトリごと消えるため。
    pub(crate) fn open_temp() -> (TempDir, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(&dir.path().join("app.db")).expect("open store");
        (dir, store)
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
}
