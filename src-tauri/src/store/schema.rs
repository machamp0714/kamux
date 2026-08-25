use rusqlite::Connection;

use crate::error::{AppError, AppResult};

/// M1-1 で確定するスキーマ版。以降のフェーズは +1 して match に 1 アーム足すだけでよい。
/// 2 = §20（M3-3: heuristics_enabled / silence_timeout_secs）
/// 3 = §29.1（M3-4: is_scratch）。この順序を入れ替えないこと（§34.1）
pub const SCHEMA_VERSION: i64 = 3;

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

/// 契約 §20 / §117.2 の DDL をそのまま写したもの（M3-3: 汎用 CLI ヒューリスティックの
/// セッション単位設定）。
///
/// **`DDL_V1` 側にこの 2 列を足してはならない。** `migrate` は新規 DB でも
/// `current = 0` から v1 → v2 を順に適用するため、`DDL_V1` が列を持っていると
/// 直後の `ALTER TABLE ADD COLUMN` が `duplicate column name` で必ず失敗し、
/// 新規 DB が 1 つも開けなくなる。
///
/// 既存行の `silence_timeout_secs` は一律 `DEFAULT 30` で埋まる
/// （`DEFAULT_SILENCE_TIMEOUT_SECS` と同値なので `UPDATE` は要らない）。
/// `heuristics_enabled` は移行前から在る行を、構築点
/// （`Session::new_backlog` → `default_heuristics_enabled`）が同じ `cli_kind` に
/// 与える値へ正規化する（契約 §117.2）。
const MIGRATION_V2: &str = r#"
ALTER TABLE sessions ADD COLUMN heuristics_enabled   INTEGER NOT NULL DEFAULT 1;
ALTER TABLE sessions ADD COLUMN silence_timeout_secs INTEGER NOT NULL DEFAULT 30;
-- 移行前から在る行を、構築点（Session::new_backlog → default_heuristics_enabled）が
-- 同じ cli_kind に与える値へ正規化する（契約 §117）。
-- 🔴 'shell' は v2 時点の default_heuristics_enabled の写しであり、ここで凍結する。
--    将来 default_heuristics_enabled が変わっても、この行を追随させてはならない
--    —— マイグレーションは履歴であって現在の方針ではない（§117.3）
UPDATE sessions SET heuristics_enabled = 0 WHERE cli_kind = 'shell';
"#;

/// 契約 §29.1 の DDL をそのまま写したもの（M3-4: スクラッチセッション）。
///
/// **`DDL_V1` 側にこの列を足してはならない。** `MIGRATION_V2` の doc コメントと
/// 同じ理由（`duplicate column name` で新規 DB が開けなくなる）。
///
/// `UPDATE` は足さない。契約 §117.4 が §117.3 の 4 手順を適用済みで、結論は
/// 「該当 0 件」——移行前の行に `create_scratch_session` 由来のものは無く
/// （version 3 と同じフェーズで新設される）、全行の構築点は
/// `Session::new_backlog`（`is_scratch: false`）であり `DEFAULT 0` と一致する。
const MIGRATION_V3: &str = r#"
ALTER TABLE sessions ADD COLUMN is_scratch INTEGER NOT NULL DEFAULT 0;
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
            3 => tx.execute_batch(MIGRATION_V3)?, // §29.1（M3-4）
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
    fn migrate_normalizes_pre_v2_shell_rows_and_leaves_the_other_cli_kinds_on_the_ddl_defaults() {
        // 移行前から在る行は、構築点（`Session::new_backlog` → `default_heuristics_enabled`）が
        // 同じ cli_kind に与える値へ正規化される（契約 §117.2 / §117.5）。4 値のうち
        // `shell` だけが 0 になる。`insert_session` は値を明示バインドするので、
        // DDL の DEFAULT とこの UPDATE が観測される経路はこのバックフィルだけである。
        let mut conn = v1_conn();
        insert_v1_session(&conn, "s_claude", "claude");
        insert_v1_session(&conn, "s_codex", "codex");
        insert_v1_session(&conn, "s_shell", "shell");
        insert_v1_session(&conn, "s_custom", "custom");

        migrate(&mut conn).expect("migrate");

        let row = |id: &str| -> (i64, i64) {
            conn.query_row(
                "SELECT heuristics_enabled, silence_timeout_secs FROM sessions WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("read back the migrated row")
        };

        // `UPDATE` を全行へ広げる変異の観測点（契約 §117.5 の項目 3）
        assert_eq!(
            row("s_claude"),
            (1, 30),
            "claude 行は DDL の DEFAULT のまま"
        );
        assert_eq!(row("s_codex"), (1, 30), "codex 行は DDL の DEFAULT のまま");
        assert_eq!(
            row("s_custom"),
            (1, 30),
            "custom 行は DDL の DEFAULT のまま"
        );
        assert_eq!(
            row("s_shell"),
            (0, 30),
            "移行前の shell 行は 0 へ正規化する（契約 §117.2）。silence_timeout_secs は触らない"
        );
    }

    /// 契約 §29.1 で `SCHEMA_VERSION` が 3 になったことで、v1 からの `migrate` は
    /// v1 → v2 → v3 の 3 反復を通るようになった。フィクスチャ（`v1_conn`）は
    /// 無変更だが、この経路の変化により旧名 `migrate_records_version_2` は
    /// 実態と食い違う（実際には 3 まで進む）ため改名する（契約に名指しの凍結
    /// 対象ではないことを `grep` で確認済み。§116.2 (c) の一般原則に従う）。
    #[test]
    fn migrate_from_v1_reaches_the_current_schema_version() {
        let mut conn = v1_conn();
        migrate(&mut conn).expect("migrate");

        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .expect("version");
        assert_eq!(version, 3);
        assert_eq!(SCHEMA_VERSION, 3, "契約 §29.1: M3-4 の版は 3");
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

    /// `DDL_V1` が全文 `IF NOT EXISTS` であることに依存した経路を通す。
    ///
    /// 契約 §46.3 落とし穴 1 は「version 1 は DDL が全文 `IF NOT EXISTS` なので、
    /// v1 のコミット後・v2 のコミット前に落ちた DB を次回の `open` が自己修復できる」
    /// という性質を明文の拠り所にしている。その性質を観測するには、**既にスキーマが
    /// 存在する DB に対してループの v1 反復を走らせる**必要がある。
    ///
    /// ここでは `schema_version` に「迷子の version 0 行」を置いてそれを作る
    /// （`COALESCE(MAX(version), 0)` は空テーブルと version 0 行を区別しないので、
    /// `current = 0` すなわち `1..=3` の 3 反復に入る。契約 §29.1 で
    /// `SCHEMA_VERSION` が 3 になったため、旧コメントの `1..=2` から更新）。
    /// v1 は既存スキーマの上を `IF NOT EXISTS` で素通りし、v2 / v3 は列がまだ
    /// 無いので成功する。
    ///
    /// `v1_conn()` を使わないのは意図的である —— あちらは版 1 を記録するため
    /// ループが `2..=3` になり、`DDL_V1` が 1 度も実行されない。
    #[test]
    fn migrate_applies_ddl_v1_over_an_existing_schema_when_the_version_row_is_stray() {
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(DDL_V1).expect("v1 ddl");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY NOT NULL);
             INSERT INTO schema_version (version) VALUES (0);",
        )
        .expect("seed a stray version 0 row");

        migrate(&mut conn).expect("v1 が既存スキーマの上を素通りしていない");

        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .expect("version");
        assert_eq!(version, 3, "迷子行から v1 → v2 → v3 を通した後の版");

        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
            .expect("count");
        assert_eq!(rows, 1, "version = 0 の迷子行が残っている");
    }

    /// 各版のコミットが記帳するのは「いま適用した版」であって、目標版
    /// （`SCHEMA_VERSION`）ではない。
    ///
    /// 新規 DB では両者の最終値が一致するので、公開面（`MAX(version)`）では区別が
    /// 付かない。区別が出るのは v1 のコミット後・v2 の失敗後の中間状態だけである
    /// —— そこで目標版が記帳されていると、次回の `open` はループが空範囲になり
    /// `ALTER TABLE` が永久に飛ぶ（2 列が無いまま `SELECT` するので起動不能になる）。
    ///
    /// v2 を確実に失敗させるため、`heuristics_enabled` を既に持つ `sessions` を
    /// 置いた DB を作る（`ALTER TABLE ADD COLUMN` に `IF NOT EXISTS` 形は無いので
    /// duplicate column name で落ちる）。
    #[test]
    fn a_failed_v2_records_version_1_not_the_target_version() {
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(DDL_V1).expect("v1 ddl");
        conn.execute_batch(
            "ALTER TABLE sessions ADD COLUMN heuristics_enabled INTEGER NOT NULL DEFAULT 1;",
        )
        .expect("make the v2 ALTER collide");

        migrate(&mut conn).expect_err("v2 は duplicate column name で失敗するはず");

        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .expect("version");
        assert_eq!(version, 1, "v1 のコミットが記帳するのは版 1 である");
        assert!(
            !columns(&conn).contains(&"silence_timeout_secs".to_owned()),
            "v2 は適用されていない —— 版 2 を記帳すると、この列が無いまま完了扱いになる"
        );
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

#[cfg(test)]
mod migration_v3_tests {
    use super::*;

    /// `migration_v2_tests::columns` と同じ実装。あちらはモジュール private
    /// なのでここで複製する。
    fn columns(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("PRAGMA table_info(sessions)")
            .expect("prepare pragma");
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .expect("query pragma");
        rows.map(|r| r.expect("column name")).collect()
    }

    /// v2 まで適用済みの DB を作る（`DDL_V1` → `MIGRATION_V2`）。`schema_version` に 2 を
    /// 記録することで、`migrate` が `(2+1)..=SCHEMA_VERSION` すなわち v3 だけを
    /// 適用する経路に入る（`migration_v2_tests::v1_conn` と同じ作法）。
    fn v2_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(DDL_V1).expect("v1 ddl");
        conn.execute_batch(MIGRATION_V2).expect("v2 migration");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY NOT NULL);
             INSERT INTO schema_version (version) VALUES (2);",
        )
        .expect("record version 2");
        conn
    }

    /// v2 の時代に既に存在していた行を 1 本作る（契約 §117.3/§117.4 の観測対象。
    /// 「移行前の行」を持つフィクスチャ）。
    fn insert_v2_session(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO projects (id, name, repo_path, created_at, updated_at)
             VALUES ('p1', 'kamux', '/Users/x/repo/kamux', 0, 0)",
            [],
        )
        .expect("insert project");
        conn.execute(
            "INSERT INTO sessions (id, project_id, title, sort_order, mode, cli_kind,
                                   created_at, updated_at)
             VALUES (?1, 'p1', 'old row', 1.0, 'in_place', 'shell', 0, 0)",
            [id],
        )
        .expect("insert v2 session");
    }

    /// 契約 §117.3 の 4 手順を実測する（§117.4）。
    /// 1. 新規行への構築点は `Session::new_backlog`（`is_scratch: false`）と
    ///    `create_scratch_session`（M3-4 Task 17、常に true）の 2 つ
    /// 2. 移行前の行に `create_scratch_session` 由来のものは無い
    ///    （version 3 と同じフェーズで新設されるコマンドのため）。したがって
    ///    移行前の行の構築点はすべて `new_backlog` であり、値は `false`
    /// 3. `DEFAULT 0` と一致し、食い違う部分集合は無い
    /// 4. 結論: 「該当 0 件」。`UPDATE` は足さない
    ///
    /// 「該当 0 件」は机上の結論に過ぎないため、移行前の行を最低 1 本持つ
    /// フィクスチャで実測する（§117.3 の 🔴。母数 0 の検査は「違反なし」と
    /// 区別が付かない）。
    #[test]
    fn migrate_leaves_pre_v3_rows_with_is_scratch_defaulted_to_zero() {
        let mut conn = v2_conn();
        insert_v2_session(&conn, "s_old");

        migrate(&mut conn).expect("migrate to v3");

        let is_scratch: i64 = conn
            .query_row(
                "SELECT is_scratch FROM sessions WHERE id = 's_old'",
                [],
                |r| r.get(0),
            )
            .expect("read back the migrated row");
        assert_eq!(
            is_scratch, 0,
            "移行前の行は DDL の DEFAULT 0 のまま（契約 §117.4: 該当 0 件）"
        );
    }

    #[test]
    fn migrate_records_version_3() {
        let mut conn = v2_conn();
        migrate(&mut conn).expect("migrate");

        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .expect("version");
        assert_eq!(version, 3);
        assert_eq!(SCHEMA_VERSION, 3, "契約 §29.1: M3-4 の版は 3");
    }

    /// `DDL_V1` 側に `is_scratch` を足してはならないという `MIGRATION_V3` の doc
    /// コメントの断定を実測で固定する（`MIGRATION_V2` の doc と同型の禁止。
    /// `MIGRATION_V2` 側は同種のテストを持たないため独立に観測する）。
    /// `DDL_V1` が列を持つと、直後の v1 → v3 のループ内で `ALTER TABLE ADD COLUMN`
    /// が `duplicate column name` を返し、新規 DB が 1 つも開けなくなる。
    #[test]
    fn a_fresh_database_opens_without_duplicate_column_errors_through_v3() {
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        migrate(&mut conn).expect("新規 DB が v1..=v3 を通せていない");

        assert!(
            columns(&conn).contains(&"is_scratch".to_owned()),
            "is_scratch が追加されていない"
        );
    }
}
