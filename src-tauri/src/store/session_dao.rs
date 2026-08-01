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
}

#[cfg(test)]
mod tests {
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
        assert!(matches!(err, crate::error::AppError::Db(_)));
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
