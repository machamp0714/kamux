use rusqlite::{params, Row};

use crate::error::{AppError, AppResult};
use crate::model::{CliKind, KanbanStatus, RuntimeState, Session, SessionMode};
use crate::store::Store;

pub(crate) const SESSION_COLUMNS: &str = "id, project_id, title, description, kanban_status, \
     sort_order, mode, branch, worktree_path, cli_kind, cli_command, claude_session_id, \
     last_runtime_state, last_runtime_error, first_started_at, archived_at, created_at, updated_at";

pub(crate) fn row_to_session(row: &Row<'_>) -> AppResult<Session> {
    let kanban_status: String = row.get("kanban_status")?;
    let mode: String = row.get("mode")?;
    let cli_kind: String = row.get("cli_kind")?;
    let last_runtime_state: String = row.get("last_runtime_state")?;

    Ok(Session {
        id: row.get("id")?,
        project_id: row.get("project_id")?,
        title: row.get("title")?,
        description: row.get("description")?,
        kanban_status: KanbanStatus::from_db_str(&kanban_status)
            .ok_or_else(|| AppError::Db(format!("unknown kanban_status in db: {kanban_status}")))?,
        sort_order: row.get("sort_order")?,
        mode: SessionMode::from_db_str(&mode)
            .ok_or_else(|| AppError::Db(format!("unknown mode in db: {mode}")))?,
        branch: row.get("branch")?,
        worktree_path: row.get("worktree_path")?,
        cli_kind: CliKind::from_db_str(&cli_kind)
            .ok_or_else(|| AppError::Db(format!("unknown cli_kind in db: {cli_kind}")))?,
        cli_command: row.get("cli_command")?,
        claude_session_id: row.get("claude_session_id")?,
        last_runtime_state: RuntimeState::from_db_str(&last_runtime_state).ok_or_else(|| {
            AppError::Db(format!(
                "unknown last_runtime_state in db: {last_runtime_state}"
            ))
        })?,
        last_runtime_error: row.get("last_runtime_error")?,
        first_started_at: row.get("first_started_at")?,
        archived_at: row.get("archived_at")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

impl Store {
    /// 列の末尾に積むための採番。空の列では MAX(sort_order) が NULL になるため
    /// COALESCE で 0.0 に落とす（契約 §3: 新規カードは列の末尾 max + 1.0）。
    pub fn next_sort_order(&self, project_id: &str, kanban_status: KanbanStatus) -> AppResult<f64> {
        let conn = self.conn()?;
        let sort_order: f64 = conn.query_row(
            "SELECT COALESCE(MAX(sort_order), 0.0) + 1.0 FROM sessions
             WHERE project_id = ?1 AND kanban_status = ?2",
            params![project_id, kanban_status.as_db_str()],
            |r| r.get(0),
        )?;
        Ok(sort_order)
    }

    /// 契約 §17: 組み立て済みの Session をそのまま 18 カラム書く。
    /// id / sort_order / タイムスタンプの決定は呼び出し側の責務。
    pub fn insert_session(&self, session: &Session) -> AppResult<Session> {
        let conn = self.conn()?;

        conn.execute(
            "INSERT INTO sessions
                (id, project_id, title, description, kanban_status, sort_order, mode,
                 branch, worktree_path, cli_kind, cli_command, claude_session_id,
                 last_runtime_state, last_runtime_error, first_started_at, archived_at,
                 created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                session.id,
                session.project_id,
                session.title,
                session.description,
                session.kanban_status.as_db_str(),
                session.sort_order,
                session.mode.as_db_str(),
                session.branch,
                session.worktree_path,
                session.cli_kind.as_db_str(),
                session.cli_command,
                session.claude_session_id,
                session.last_runtime_state.as_db_str(),
                session.last_runtime_error,
                session.first_started_at,
                session.archived_at,
                session.created_at,
                session.updated_at,
            ],
        )?;

        Ok(session.clone())
    }

    /// 契約 §17: 不在は Option ではなく AppError::NotFound で表す。
    pub fn get_session(&self, id: &str) -> AppResult<Session> {
        let conn = self.conn()?;
        fetch_session(&conn, id)
    }

    /// 契約 §17: 表示順は kanban_status, sort_order, id の順で確定させる。
    /// sort_order に一意制約が無いため、id までタイブレークに含める。
    pub fn list_sessions(
        &self,
        project_id: &str,
        include_archived: bool,
    ) -> AppResult<Vec<Session>> {
        let conn = self.conn()?;
        let archived_filter = if include_archived {
            ""
        } else {
            "AND archived_at IS NULL"
        };
        let sql = format!(
            "SELECT {SESSION_COLUMNS} FROM sessions
             WHERE project_id = ?1 {archived_filter}
             ORDER BY kanban_status ASC, sort_order ASC, id ASC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_and_then(params![project_id], row_to_session)?;
        rows.collect()
    }
}

/// 単体取得。呼び出し側が既に Connection のロックを持っているときに使う。
pub(crate) fn fetch_session(conn: &rusqlite::Connection, id: &str) -> AppResult<Session> {
    let sql = format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_and_then(params![id], row_to_session)?;
    match rows.next() {
        Some(session) => session,
        None => Err(AppError::NotFound(id.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::params;

    use crate::model::{CliKind, KanbanStatus, RuntimeState, Session, SessionMode};
    use crate::store::session_dao::{row_to_session, SESSION_COLUMNS};
    use crate::store::test_support::{insert_test_session, open_temp};
    use crate::store::Store;

    fn project(store: &Store) -> String {
        store
            .insert_project("kamux", "/Users/x/repo/kamux", CliKind::Claude)
            .expect("project")
            .id
    }

    #[test]
    fn next_sort_order_starts_at_1_and_increments() {
        let (_dir, store) = open_temp();
        let pid = project(&store);

        assert_eq!(
            store
                .next_sort_order(&pid, KanbanStatus::Backlog)
                .expect("first"),
            1.0,
            "空の列では MAX が NULL になる"
        );

        insert_test_session(&store, &pid, "a");
        assert_eq!(
            store
                .next_sort_order(&pid, KanbanStatus::Backlog)
                .expect("second"),
            2.0
        );

        insert_test_session(&store, &pid, "b");
        assert_eq!(
            store
                .next_sort_order(&pid, KanbanStatus::Backlog)
                .expect("third"),
            3.0
        );
    }

    #[test]
    fn next_sort_order_is_scoped_to_project_and_status() {
        let (_dir, store) = open_temp();
        let p1 = project(&store);
        let p2 = store
            .insert_project("other", "/Users/x/repo/other", CliKind::Claude)
            .expect("p2")
            .id;

        insert_test_session(&store, &p1, "a");
        insert_test_session(&store, &p1, "b");

        assert_eq!(
            store
                .next_sort_order(&p2, KanbanStatus::Backlog)
                .expect("p2"),
            1.0,
            "別プロジェクトの採番に引きずられない"
        );
        assert_eq!(
            store
                .next_sort_order(&p1, KanbanStatus::Review)
                .expect("review"),
            1.0,
            "別の列の採番に引きずられない"
        );
    }

    #[test]
    fn insert_session_persists_every_column() {
        let (_dir, store) = open_temp();
        let pid = project(&store);

        let session = Session::new_backlog(
            &pid,
            "fix login",
            "壊れたログインを直す",
            SessionMode::Worktree,
            Some("session/fix-login".to_owned()),
            CliKind::Custom,
            Some("bun run agent".to_owned()),
            2.5,
            1700000000000,
        );
        let returned = store.insert_session(&session).expect("insert");
        assert_eq!(returned.id, session.id, "渡した Session の複製を返す");

        let conn = store.conn().expect("conn");
        let (title, status, sort_order, mode, branch, cli_kind, cli_command, state): (
            String,
            String,
            f64,
            String,
            Option<String>,
            String,
            Option<String>,
            String,
        ) = conn
            .query_row(
                "SELECT title, kanban_status, sort_order, mode, branch, cli_kind,
                        cli_command, last_runtime_state
                 FROM sessions WHERE id = ?1",
                [&session.id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                    ))
                },
            )
            .expect("row");

        assert_eq!(title, "fix login");
        assert_eq!(status, "backlog");
        assert_eq!(sort_order, 2.5);
        assert_eq!(mode, "worktree");
        assert_eq!(branch.as_deref(), Some("session/fix-login"));
        assert_eq!(cli_kind, "custom");
        assert_eq!(cli_command.as_deref(), Some("bun run agent"));
        assert_eq!(state, "idle");
    }

    #[test]
    fn insert_session_rejects_unknown_project() {
        let (_dir, store) = open_temp();
        let session = Session::new_backlog(
            "no-such-project",
            "t",
            "",
            SessionMode::InPlace,
            None,
            CliKind::Shell,
            None,
            1.0,
            1,
        );
        let err = store
            .insert_session(&session)
            .expect_err("FK 制約で失敗するはず");
        match err {
            crate::error::AppError::Db(message) => {
                assert!(
                    message.to_lowercase().contains("foreign key"),
                    "FK 違反以外の理由で失敗している可能性がある: {message}"
                );
            }
            other => panic!("expected AppError::Db, got {other:?}"),
        }
    }

    #[test]
    fn insert_test_session_helper_numbers_rows_in_order() {
        let (_dir, store) = open_temp();
        let pid = project(&store);

        let a = insert_test_session(&store, &pid, "a");
        let b = insert_test_session(&store, &pid, "b");

        assert_eq!(a.sort_order, 1.0);
        assert_eq!(b.sort_order, 2.0);
        assert_eq!(a.kanban_status, KanbanStatus::Backlog);
        assert_eq!(a.last_runtime_state, RuntimeState::Idle);
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn row_to_session_round_trips_every_column_including_previously_dropped_fields() {
        let (_dir, store) = open_temp();
        let pid = project(&store);

        // 契約 §37.2: last_runtime_error がフィールド並びから欠落した事故を再演しない
        // ための往復テスト。first_started_at / archived_at も同様に明示的に見る。
        let session = Session::new_backlog(
            &pid,
            "fix login",
            "壊れたログインを直す",
            SessionMode::Worktree,
            Some("session/fix-login".to_owned()),
            CliKind::Custom,
            Some("bun run agent".to_owned()),
            2.5,
            1700000000000,
        );
        store.insert_session(&session).expect("insert");

        {
            let conn = store.conn().expect("conn");
            conn.execute(
                "UPDATE sessions SET last_runtime_state = 'error',
                     last_runtime_error = 'boom', first_started_at = 1700000001000,
                     archived_at = 1700000002000
                 WHERE id = ?1",
                [&session.id],
            )
            .expect("update");
        }

        let conn = store.conn().expect("conn");
        let sql = format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE id = ?1");
        let mut stmt = conn.prepare(&sql).expect("prepare");
        let mut rows = stmt
            .query_and_then([&session.id], row_to_session)
            .expect("query");
        let fetched = rows.next().expect("row").expect("row_to_session");

        assert_eq!(fetched.id, session.id);
        assert_eq!(fetched.project_id, pid);
        assert_eq!(fetched.title, "fix login");
        assert_eq!(fetched.description, "壊れたログインを直す");
        assert_eq!(fetched.kanban_status, KanbanStatus::Backlog);
        assert_eq!(fetched.sort_order, 2.5);
        assert_eq!(fetched.mode, SessionMode::Worktree);
        assert_eq!(fetched.branch.as_deref(), Some("session/fix-login"));
        assert_eq!(fetched.worktree_path, None);
        assert_eq!(fetched.cli_kind, CliKind::Custom);
        assert_eq!(fetched.cli_command.as_deref(), Some("bun run agent"));
        assert_eq!(fetched.claude_session_id, None);
        assert_eq!(fetched.last_runtime_state, RuntimeState::Error);
        assert_eq!(
            fetched.last_runtime_error.as_deref(),
            Some("boom"),
            "§37.2 の事故: この列が並びから欠落していた"
        );
        assert_eq!(fetched.first_started_at, Some(1700000001000));
        assert_eq!(fetched.archived_at, Some(1700000002000));
        assert_eq!(fetched.created_at, 1700000000000);
    }

    #[test]
    fn insert_session_writes_every_field_to_its_own_bound_position() {
        // `new_backlog` は 5 フィールドを常に None に固定するため使わない。
        // INSERT 列 <-> params! の対応が 1 か所でもズレたら必ず落ちるよう、
        // 18 フィールド全部に相互に区別できる非 NULL 値を入れる
        // （特に created_at != updated_at）。往復テスト
        // (row_to_session_round_trips_...) は生 UPDATE で値を入れており
        // insert_session の位置ズレは検出できないため、これで補う。
        let (_dir, store) = open_temp();
        let pid = project(&store);

        let session = Session {
            id: "sid-value".to_owned(),
            project_id: pid.clone(),
            title: "title-value".to_owned(),
            description: "description-value".to_owned(),
            kanban_status: KanbanStatus::Review,
            sort_order: 7.5,
            mode: SessionMode::Worktree,
            branch: Some("branch-value".to_owned()),
            worktree_path: Some("worktree-path-value".to_owned()),
            cli_kind: CliKind::Custom,
            cli_command: Some("cli-command-value".to_owned()),
            claude_session_id: Some("claude-session-id-value".to_owned()),
            last_runtime_state: RuntimeState::Error,
            last_runtime_error: Some("last-runtime-error-value".to_owned()),
            first_started_at: Some(111),
            archived_at: Some(222),
            created_at: 333,
            updated_at: 444,
        };
        store.insert_session(&session).expect("insert");

        let conn = store.conn().expect("conn");
        let sql = format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE id = ?1");
        let mut stmt = conn.prepare(&sql).expect("prepare");
        let mut rows = stmt
            .query_and_then(["sid-value"], row_to_session)
            .expect("query");
        let fetched = rows.next().expect("row").expect("row_to_session");

        assert_eq!(fetched.id, "sid-value");
        assert_eq!(fetched.project_id, pid);
        assert_eq!(fetched.title, "title-value");
        assert_eq!(fetched.description, "description-value");
        assert_eq!(fetched.kanban_status, KanbanStatus::Review);
        assert_eq!(fetched.sort_order, 7.5);
        assert_eq!(fetched.mode, SessionMode::Worktree);
        assert_eq!(fetched.branch.as_deref(), Some("branch-value"));
        assert_eq!(
            fetched.worktree_path.as_deref(),
            Some("worktree-path-value")
        );
        assert_eq!(fetched.cli_kind, CliKind::Custom);
        assert_eq!(fetched.cli_command.as_deref(), Some("cli-command-value"));
        assert_eq!(
            fetched.claude_session_id.as_deref(),
            Some("claude-session-id-value")
        );
        assert_eq!(fetched.last_runtime_state, RuntimeState::Error);
        assert_eq!(
            fetched.last_runtime_error.as_deref(),
            Some("last-runtime-error-value")
        );
        assert_eq!(fetched.first_started_at, Some(111));
        assert_eq!(fetched.archived_at, Some(222));
        assert_eq!(fetched.created_at, 333);
        assert_eq!(
            fetched.updated_at, 444,
            "created_at と入れ替わっていないこと"
        );
    }

    #[test]
    fn row_to_session_reports_unknown_enum_value_as_db_error_instead_of_panicking() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        insert_test_session(&store, &pid, "a");

        {
            let conn = store.conn().expect("conn");
            conn.execute("UPDATE sessions SET kanban_status = 'archived'", [])
                .expect("update");
        }

        let conn = store.conn().expect("conn");
        let sql = format!("SELECT {SESSION_COLUMNS} FROM sessions");
        let mut stmt = conn.prepare(&sql).expect("prepare");
        let err = stmt
            .query_and_then([], row_to_session)
            .expect("query")
            .next()
            .expect("row")
            .expect_err("未知の kanban_status は握り潰さない");
        match err {
            crate::error::AppError::Db(message) => assert!(message.contains("archived")),
            other => panic!("expected AppError::Db, got {other:?}"),
        }
    }

    #[test]
    fn get_session_returns_the_row_or_not_found() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let created = insert_test_session(&store, &pid, "a");

        let fetched = store.get_session(&created.id).expect("get");
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.title, "a");
        assert_eq!(fetched.description, "");
        assert_eq!(fetched.sort_order, created.sort_order);
        assert_eq!(fetched.cli_kind, CliKind::Shell, "列挙型が往復している");

        let err = store.get_session("nope").expect_err("無い ID");
        match err {
            crate::error::AppError::NotFound(id) => assert_eq!(id, "nope"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn list_sessions_is_scoped_to_project_and_ordered_by_sort_order() {
        let (_dir, store) = open_temp();
        let p1 = project(&store);
        let p2 = store
            .insert_project("other", "/Users/x/repo/other", CliKind::Claude)
            .expect("p2")
            .id;

        let a = insert_test_session(&store, &p1, "a");
        let b = insert_test_session(&store, &p1, "b");
        insert_test_session(&store, &p2, "other");

        let list = store.list_sessions(&p1, false).expect("list");
        assert_eq!(list.len(), 2, "他プロジェクトのセッションが混ざっている");
        assert_eq!(list[0].id, a.id);
        assert_eq!(list[1].id, b.id);
    }

    #[test]
    fn list_sessions_orders_by_sort_order_not_insertion_order() {
        // 3 番目に挿入した行の sort_order を最小に書き換え、それが先頭に来ることを見て
        // ORDER BY sort_order が効いていることを確認する。
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let a = insert_test_session(&store, &pid, "a");
        let b = insert_test_session(&store, &pid, "b");
        let c = insert_test_session(&store, &pid, "c");

        {
            let conn = store.conn().expect("conn");
            conn.execute(
                "UPDATE sessions SET sort_order = 0.5 WHERE id = ?1",
                [&c.id],
            )
            .expect("reorder");
        }

        let list = store.list_sessions(&pid, false).expect("list");
        assert_eq!(
            list.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
            vec![c.id, a.id, b.id],
            "sort_order の昇順になっていない"
        );
    }

    #[test]
    fn list_sessions_breaks_sort_order_ties_by_id() {
        // 契約 §17: sort_order に一意制約は無いため、同値になったときは id で
        // タイブレークする。(project_id, kanban_status, sort_order) の複合インデックスは
        // sort_order までしかカバーしないため、id ASC を明示しないとこの並びは
        // 保証されない。
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let a = insert_test_session(&store, &pid, "a");
        let b = insert_test_session(&store, &pid, "b");

        {
            let conn = store.conn().expect("conn");
            conn.execute(
                "UPDATE sessions SET sort_order = ?1 WHERE id = ?2",
                params![a.sort_order, &b.id],
            )
            .expect("tie sort_order");
        }

        let mut expected_ids = vec![a.id.clone(), b.id.clone()];
        expected_ids.sort();

        let list = store.list_sessions(&pid, false).expect("list");
        assert_eq!(
            list.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
            expected_ids,
            "sort_order 同値時に id でタイブレークされていない"
        );
    }

    #[test]
    fn list_sessions_hides_archived_unless_requested() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let a = insert_test_session(&store, &pid, "a");
        insert_test_session(&store, &pid, "b");

        {
            let conn = store.conn().expect("conn");
            conn.execute("UPDATE sessions SET archived_at = 1 WHERE id = ?1", [&a.id])
                .expect("archive");
        }

        assert_eq!(store.list_sessions(&pid, false).expect("list").len(), 1);
        assert_eq!(store.list_sessions(&pid, true).expect("list").len(), 2);
    }

    #[test]
    fn list_sessions_returns_empty_for_unknown_project() {
        let (_dir, store) = open_temp();
        assert!(store.list_sessions("nope", false).expect("list").is_empty());
    }

    #[test]
    fn next_sort_order_counts_archived_rows_to_avoid_reuse_on_unarchive() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let session = insert_test_session(&store, &pid, "a");

        {
            let conn = store.conn().expect("conn");
            conn.execute(
                "UPDATE sessions SET archived_at = 1700000000000 WHERE id = ?1",
                [&session.id],
            )
            .expect("archive");
        }

        assert_eq!(
            store
                .next_sort_order(&pid, KanbanStatus::Backlog)
                .expect("next"),
            2.0,
            "アーカイブ済みでも MAX に含めないと、解除時に同値衝突する（M3-4）"
        );
    }
}
