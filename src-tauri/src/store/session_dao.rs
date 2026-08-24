use rusqlite::{params, Row};

use crate::error::{AppError, AppResult};
use crate::model::{CliKind, KanbanStatus, RuntimeState, Session, SessionMode, SessionPatch};
use crate::session::heuristics::validate::validate_silence_timeout_secs;
use crate::store::{now_ms, Store};

pub(crate) const SESSION_COLUMNS: &str = "id, project_id, title, description, kanban_status, \
     sort_order, mode, branch, worktree_path, cli_kind, cli_command, claude_session_id, \
     last_runtime_state, last_runtime_error, first_started_at, heuristics_enabled, \
     silence_timeout_secs, archived_at, created_at, updated_at";

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
        heuristics_enabled: row.get("heuristics_enabled")?,
        silence_timeout_secs: row.get("silence_timeout_secs")?,
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

    /// 契約 §17: 組み立て済みの Session をそのまま 20 カラム書く。
    /// id / sort_order / タイムスタンプの決定は呼び出し側の責務。
    ///
    /// heuristics_enabled / silence_timeout_secs も **渡された値をそのまま**書く。
    /// ここで cli_kind から既定値を再計算すると、Session が false を持っていても
    /// DB には true が入る（構造体と DB の split-brain）。既定値の決定は
    /// `Session::new_backlog` だけの責務である（契約 §20 / 設計 §4.6）。
    pub fn insert_session(&self, session: &Session) -> AppResult<Session> {
        let conn = self.conn()?;

        conn.execute(
            "INSERT INTO sessions
                (id, project_id, title, description, kanban_status, sort_order, mode,
                 branch, worktree_path, cli_kind, cli_command, claude_session_id,
                 last_runtime_state, last_runtime_error, first_started_at,
                 heuristics_enabled, silence_timeout_secs, archived_at,
                 created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                     ?18, ?19, ?20)",
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
                session.heuristics_enabled,
                session.silence_timeout_secs,
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

    /// 部分更新。動的 SQL を組まず「読む → Rust 上で patch を当てる → 7 カラム書く」で行う。
    /// 単一 Mutex 配下で他の書き手がいないため、この read-modify-write に競合はない。
    /// branch / worktree_path / claude_session_id / last_runtime_state /
    /// last_runtime_error / first_started_at には触らない
    /// （契約 §17 のセッタ群が書いた値を潰さないため。Task 11 に回帰テストがある）。
    pub fn update_session(&self, id: &str, patch: &SessionPatch) -> AppResult<Session> {
        let conn = self.conn()?;
        let mut session = fetch_session(&conn, id)?;

        if let Some(title) = &patch.title {
            session.title = title.clone();
        }
        if let Some(description) = &patch.description {
            session.description = description.clone();
        }
        if let Some(kanban_status) = patch.kanban_status {
            session.kanban_status = kanban_status;
        }
        if let Some(sort_order) = patch.sort_order {
            session.sort_order = sort_order;
        }
        // Some(None) は「null が明示された」＝ アーカイブ解除。None は「不在」＝ 変更しない。
        if let Some(archived_at) = patch.archived_at {
            session.archived_at = archived_at;
        }
        // 契約 §20（M3-3）。範囲外は丸めずに拒否する（設計 §4.5）。
        // `?` で UPDATE の前に抜けるので、同じ patch の他のフィールドも書かれない。
        if let Some(heuristics_enabled) = patch.heuristics_enabled {
            session.heuristics_enabled = heuristics_enabled;
        }
        if let Some(silence_timeout_secs) = patch.silence_timeout_secs {
            session.silence_timeout_secs = validate_silence_timeout_secs(silence_timeout_secs)?;
        }
        session.updated_at = now_ms();

        conn.execute(
            "UPDATE sessions
             SET title = ?1, description = ?2, kanban_status = ?3, sort_order = ?4,
                 archived_at = ?5, heuristics_enabled = ?6, silence_timeout_secs = ?7,
                 updated_at = ?8
             WHERE id = ?9",
            params![
                session.title,
                session.description,
                session.kanban_status.as_db_str(),
                session.sort_order,
                session.archived_at,
                session.heuristics_enabled,
                session.silence_timeout_secs,
                session.updated_at,
                session.id,
            ],
        )?;

        Ok(session)
    }

    /// worktree 作成後に branch / worktree_path を確定させる（M1-4）。
    pub fn set_worktree(&self, id: &str, branch: &str, worktree_path: &str) -> AppResult<()> {
        let conn = self.conn()?;
        let affected = conn.execute(
            "UPDATE sessions SET branch = ?1, worktree_path = ?2, updated_at = ?3 WHERE id = ?4",
            params![branch, worktree_path, now_ms(), id],
        )?;
        if affected == 0 {
            return Err(AppError::NotFound(id.to_owned()));
        }
        Ok(())
    }

    /// worktree 掃除後に `worktree_path` を更新する（M3-4）。`branch` には一切触れない
    /// （契約 §13「削除時の規則」: 削除成功時は worktree_path を NULL にし、branch は変更しない）。
    pub fn set_worktree_path(&self, id: &str, path: Option<&str>) -> AppResult<()> {
        let conn = self.conn()?;
        let affected = conn.execute(
            "UPDATE sessions SET worktree_path = ?1, updated_at = ?2 WHERE id = ?3",
            params![path, now_ms(), id],
        )?;
        if affected == 0 {
            return Err(AppError::NotFound(id.to_owned()));
        }
        Ok(())
    }

    /// SessionStart hook で捕捉した ID を保存（M2-2）。--continue 経路では上書きになる。
    pub fn set_claude_session_id(&self, id: &str, claude_session_id: &str) -> AppResult<()> {
        let conn = self.conn()?;
        let affected = conn.execute(
            "UPDATE sessions SET claude_session_id = ?1, updated_at = ?2 WHERE id = ?3",
            params![claude_session_id, now_ms(), id],
        )?;
        if affected == 0 {
            return Err(AppError::NotFound(id.to_owned()));
        }
        Ok(())
    }

    /// 無効な claude_session_id を破棄する（第1部 §4.2 の回復経路。M2-4）。
    /// --continue フォールバックで再取得するまでの間、null に戻しておくことで
    /// 次回の再開判定（`resume_plan`）が誤って `ClaudeResume` に乗らないようにする。
    pub fn clear_claude_session_id(&self, id: &str) -> AppResult<()> {
        let conn = self.conn()?;
        let affected = conn.execute(
            "UPDATE sessions SET claude_session_id = NULL, updated_at = ?1 WHERE id = ?2",
            params![now_ms(), id],
        )?;
        if affected == 0 {
            return Err(AppError::NotFound(id.to_owned()));
        }
        Ok(())
    }

    /// runtime_state の表示復元用ヒントを書く（M2-1）。
    /// DAO は Interrupted を弾かない。付与経路の制限（契約 §2）は SessionManager の責務。
    /// error 以外へ遷移したら last_runtime_error を NULL に戻す（契約 §17）。
    /// updated_at は動かさない —— システム導出値であり「最終更新順」を汚さないため（契約 §38.2）。
    pub fn set_last_runtime_state(&self, id: &str, state: RuntimeState) -> AppResult<()> {
        let conn = self.conn()?;
        let affected = conn.execute(
            // ?1 を 2 か所で使い回さない(番号付きプレースホルダの再利用は SQLite では
            // 合法だが、rusqlite の parameter_count と params! の要素数の対応が読みにくい)
            "UPDATE sessions
                SET last_runtime_state = ?1,
                    last_runtime_error = CASE WHEN ?2 = 'error' THEN last_runtime_error ELSE NULL END
              WHERE id = ?3",
            params![state.as_db_str(), state.as_db_str(), id],
        )?;
        if affected == 0 {
            return Err(AppError::NotFound(id.to_owned()));
        }
        Ok(())
    }

    /// error 状態への遷移時に生 stderr を保存する（M2-1。契約 §17 / §38.1）。
    /// last_runtime_state = 'error' と last_runtime_error を 1 文で書くので、
    /// 「error なのにメッセージが無い」中間状態が観測されない。
    /// updated_at は動かさない（契約 §38.2）。
    pub fn set_runtime_error(&self, id: &str, message: &str) -> AppResult<()> {
        let conn = self.conn()?;
        let affected = conn.execute(
            "UPDATE sessions SET last_runtime_state = 'error', last_runtime_error = ?1 WHERE id = ?2",
            params![message, id],
        )?;
        if affected == 0 {
            return Err(AppError::NotFound(id.to_owned()));
        }
        Ok(())
    }

    /// 最初の PTY spawn 成功時刻を 1 度だけ記録する（M2-1。契約 §34.4）。
    /// 冪等性は SQL 側で担保する。2 度目以降と不在の id はどちらも 0 行更新で Ok(())。
    /// updated_at は動かさない。
    pub fn mark_first_started(&self, id: &str, now: i64) -> AppResult<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sessions SET first_started_at = ?2
             WHERE id = ?1 AND first_started_at IS NULL",
            params![id, now],
        )?;
        Ok(())
    }

    /// 起動時正規化用（M2-1）。プロジェクトを跨いで全件返す。
    /// メンテナンス処理なので archived_at で絞らない。
    pub fn list_sessions_by_last_runtime_state(
        &self,
        state: RuntimeState,
    ) -> AppResult<Vec<Session>> {
        let conn = self.conn()?;
        let sql = format!(
            "SELECT {SESSION_COLUMNS} FROM sessions
             WHERE last_runtime_state = ?1
             ORDER BY created_at ASC, id ASC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_and_then(params![state.as_db_str()], row_to_session)?;
        rows.collect()
    }

    /// 契約 §7.4 / §49.3〜§49.5: DnD の並び替えを 1 トランザクションで原子的に行う。
    /// to_index は「移動対象自身を除いた移動先列 L」への挿入位置（契約 §49.3.2）。
    /// 戻り値は移動先の列のみ（契約 §49.4）。移動元の列は 1 行も変化しないので返さない。
    pub fn move_session(
        &self,
        id: &str,
        to_status: KanbanStatus,
        to_index: usize,
    ) -> AppResult<Vec<Session>> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;

        // 1. 対象行を取る（無ければ NotFound。列の走査より先に判定する）。
        //    project_id はこれでしか得られない —— カンバンの列はプロジェクト単位なので、
        //    L と戻り値のクエリを project_id で絞らないと他プロジェクトの行が混ざる
        //    （next_sort_order / list_sessions と同じ絞り込み。§29.4 の is_scratch と
        //    同じ論法の導出）。
        let project_id = fetch_session(&tx, id)?.project_id;

        // 2. L = 移動先の列（対象自身とアーカイブ済みを除く）の (id, sort_order) を
        //    sort_order ASC, id ASC で読む
        let l: Vec<(String, f64)> = {
            let mut stmt = tx.prepare(
                "SELECT id, sort_order FROM sessions
                 WHERE project_id = ?1 AND kanban_status = ?2
                       AND archived_at IS NULL AND id != ?3
                 ORDER BY sort_order ASC, id ASC",
            )?;
            let rows = stmt.query_map(params![project_id, to_status.as_db_str(), id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        let clamped_index = to_index.min(l.len());

        // 3. mid を採番する（契約 §7.4 の 3 分岐）
        let mut mid = compute_mid(&l, clamped_index);

        // 4. 枯渇（mid が両隣のどちらかと厳密に等しい。契約 §49.3.1）なら、
        //    「target を挿入した後の移動先の列」全体を 1.0, 2.0, 3.0, ... へ振り直す
        //    （M1-2-kanban.md:222 の旧 TS 実装をサーバへ移した形）。**L だけを
        //    1..L.len() に振り直してから中点を取り直す読み方は誤り**（このケースだと
        //    中点が 1.5 になり、決定的テストの期待値 2.0 と食い違う）。
        //    sort_order だけを UPDATE する。updated_at は動かさない（契約 §49.5）。
        //    枯渇は to_index が中間（0 < clamped_index < L.len()）のときにしか起きない
        //    —— 端への挿入は常に ±1.0 で新しい値を作れる。
        if clamped_index > 0 && clamped_index < l.len() {
            let prev = l[clamped_index - 1].1;
            let next = l[clamped_index].1;
            if mid == prev || mid == next {
                for (i, (row_id, _)) in l.iter().enumerate() {
                    let new_value = if i < clamped_index {
                        (i + 1) as f64
                    } else {
                        (i + 2) as f64
                    };
                    tx.execute(
                        "UPDATE sessions SET sort_order = ?1 WHERE id = ?2",
                        params![new_value, row_id],
                    )?;
                }
                mid = (clamped_index + 1) as f64;
            }
        }

        // 5. 対象行を UPDATE（kanban_status / sort_order / updated_at = now_ms()）
        let now = now_ms();
        tx.execute(
            "UPDATE sessions
             SET kanban_status = ?1, sort_order = ?2, updated_at = ?3
             WHERE id = ?4",
            params![to_status.as_db_str(), mid, now, id],
        )?;

        // 6. 移動先の列を SELECT して返す（ORDER BY sort_order ASC, id ASC）。
        //    アーカイブ済みは含めない
        let column: Vec<Session> = {
            let sql = format!(
                "SELECT {SESSION_COLUMNS} FROM sessions
                 WHERE project_id = ?1 AND kanban_status = ?2 AND archived_at IS NULL
                 ORDER BY sort_order ASC, id ASC"
            );
            let mut stmt = tx.prepare(&sql)?;
            let rows =
                stmt.query_and_then(params![project_id, to_status.as_db_str()], row_to_session)?;
            rows.collect::<AppResult<Vec<_>>>()?
        };

        tx.commit()?;
        Ok(column)
    }
}

/// 契約 §7.4 の採番アルゴリズム本体。l は移動先列の (id, sort_order) を
/// sort_order ASC, id ASC で並べたもの（移動対象自身とアーカイブ済みを除く）。
fn compute_mid(l: &[(String, f64)], to_index: usize) -> f64 {
    if l.is_empty() {
        1.0
    } else if to_index == 0 {
        l[0].1 - 1.0
    } else if to_index >= l.len() {
        l[l.len() - 1].1 + 1.0
    } else {
        (l[to_index - 1].1 + l[to_index].1) / 2.0
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
    use crate::error::AppError;
    use crate::model::{CliKind, KanbanStatus, RuntimeState, Session, SessionMode, SessionPatch};
    use crate::store::session_dao::{row_to_session, SESSION_COLUMNS};
    use crate::store::test_support::{insert_test_session, open_temp};
    use crate::store::{now_ms, Store};

    fn project(store: &Store) -> String {
        store
            .insert_project("kamux", "/Users/x/repo/kamux", CliKind::Claude)
            .expect("project")
            .id
    }

    fn patch_from_json(json: &str) -> SessionPatch {
        serde_json::from_str(json).expect("deserialize patch")
    }

    /// created_at / updated_at をセンチネル値 1 に固定した Session を作る。
    /// `now_ms()`（約 1.7e12）と絶対に衝突しない値にすることで、セッタが
    /// updated_at を動かすかどうか（契約 §38.2）を `sleep` に頼らず決定的に検証できる。
    /// `insert_test_session` は `now_ms()` を使うため、この用途には使えない。
    fn session_with_sentinel_updated_at(project_id: &str, id: &str) -> Session {
        Session {
            id: id.to_owned(),
            project_id: project_id.to_owned(),
            title: id.to_owned(),
            description: String::new(),
            kanban_status: KanbanStatus::Backlog,
            sort_order: 1.0,
            mode: SessionMode::InPlace,
            branch: None,
            worktree_path: None,
            cli_kind: CliKind::Shell,
            cli_command: None,
            claude_session_id: None,
            last_runtime_state: RuntimeState::Idle,
            last_runtime_error: None,
            first_started_at: None,
            heuristics_enabled: false,
            silence_timeout_secs: 30,
            archived_at: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    /// insert_test_session は description が空なので、description の不変性を
    /// 検証するテスト専用に中身を指定できるヘルパを用意する。
    fn insert_session_with_description(
        store: &Store,
        project_id: &str,
        title: &str,
        description: &str,
    ) -> Session {
        let sort_order = store
            .next_sort_order(project_id, KanbanStatus::Backlog)
            .expect("next_sort_order");
        let session = Session::new_backlog(
            project_id,
            title,
            description,
            SessionMode::InPlace,
            None,
            CliKind::Shell,
            None,
            sort_order,
            now_ms(),
        );
        store.insert_session(&session).expect("insert_session")
    }

    #[test]
    fn update_session_changes_only_the_given_fields() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let s = insert_session_with_description(&store, &pid, "before", "keep me");

        let updated = store
            .update_session(&s.id, &patch_from_json(r#"{"title":"after"}"#))
            .expect("update");

        assert_eq!(updated.title, "after");
        assert_eq!(
            updated.description, "keep me",
            "指定していないフィールドは不変"
        );
        assert_eq!(updated.kanban_status, KanbanStatus::Backlog);
        assert_eq!(updated.sort_order, s.sort_order);
        assert_eq!(
            store.get_session(&s.id).expect("get").title,
            "after",
            "DB に永続化されている"
        );
    }

    #[test]
    fn update_session_moves_card_between_columns() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let s = insert_test_session(&store, &pid, "a");

        let updated = store
            .update_session(
                &s.id,
                &patch_from_json(r#"{"kanban_status":"in_progress","sort_order":1.5}"#),
            )
            .expect("update");

        assert_eq!(updated.kanban_status, KanbanStatus::InProgress);
        assert_eq!(updated.sort_order, 1.5);
    }

    #[test]
    fn update_session_bumps_updated_at_but_not_created_at() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let s = insert_test_session(&store, &pid, "a");

        std::thread::sleep(std::time::Duration::from_millis(3));
        let updated = store
            .update_session(&s.id, &patch_from_json(r#"{"title":"b"}"#))
            .expect("update");

        assert_eq!(updated.created_at, s.created_at);
        assert!(
            updated.updated_at > s.updated_at,
            "updated_at が進んでいない"
        );
    }

    #[test]
    fn update_session_handles_archived_at_absent_null_and_value() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let s = insert_test_session(&store, &pid, "a");

        // 数値 = アーカイブ
        let archived = store
            .update_session(&s.id, &patch_from_json(r#"{"archived_at":1700000000000}"#))
            .expect("archive");
        assert_eq!(archived.archived_at, Some(1700000000000));

        // 不在 = 変更しない
        let untouched = store
            .update_session(&s.id, &patch_from_json(r#"{"title":"b"}"#))
            .expect("untouched");
        assert_eq!(
            untouched.archived_at,
            Some(1700000000000),
            "不在で勝手に解除された"
        );

        // null = アーカイブ解除
        let restored = store
            .update_session(&s.id, &patch_from_json(r#"{"archived_at":null}"#))
            .expect("restore");
        assert_eq!(restored.archived_at, None);
        assert_eq!(
            store.get_session(&s.id).expect("get").archived_at,
            None,
            "DB 上でクリアされていない（戻り値だけでは検出できない）"
        );
    }

    #[test]
    fn update_session_does_not_overwrite_columns_owned_by_the_runtime_setters() {
        // update_session が触ってよいのは title / description / kanban_status /
        // sort_order / archived_at / heuristics_enabled / silence_timeout_secs の
        // 7 カラムのみ（後ろ 2 つは M3-3）。branch / worktree_path /
        // claude_session_id / last_runtime_state / last_runtime_error /
        // first_started_at は Task 11 のセッタ群が書く値であり、ここで潰すと
        // それらの回帰テストより先にサイレントにデータが失われる。
        //
        // update_session は fetch_session で読んだ Session をそのまま返すため、
        // 戻り値だけを見ると SQL が誤って書き潰していても検出できない。
        // 必ず get_session で読み直した結果をアサートする。
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let session = Session {
            id: "sid-preserve".to_owned(),
            project_id: pid,
            title: "before".to_owned(),
            description: "before-description".to_owned(),
            kanban_status: KanbanStatus::Backlog,
            sort_order: 1.0,
            mode: SessionMode::Worktree,
            branch: Some("branch-value".to_owned()),
            worktree_path: Some("worktree-path-value".to_owned()),
            cli_kind: CliKind::Custom,
            cli_command: Some("cli-command-value".to_owned()),
            claude_session_id: Some("claude-session-id-value".to_owned()),
            last_runtime_state: RuntimeState::Error,
            last_runtime_error: Some("last-runtime-error-value".to_owned()),
            first_started_at: Some(111),
            heuristics_enabled: true,
            silence_timeout_secs: 30,
            archived_at: None,
            created_at: 1,
            updated_at: 1,
        };
        store.insert_session(&session).expect("insert");

        store
            .update_session(
                &session.id,
                &patch_from_json(r#"{"title":"after","kanban_status":"in_progress"}"#),
            )
            .expect("update");

        let reloaded = store.get_session(&session.id).expect("get");
        assert_eq!(reloaded.title, "after", "更新対象は反映される");
        assert_eq!(reloaded.kanban_status, KanbanStatus::InProgress);
        assert_eq!(
            reloaded.branch.as_deref(),
            Some("branch-value"),
            "branch を潰した"
        );
        assert_eq!(
            reloaded.worktree_path.as_deref(),
            Some("worktree-path-value"),
            "worktree_path を潰した"
        );
        assert_eq!(
            reloaded.claude_session_id.as_deref(),
            Some("claude-session-id-value"),
            "claude_session_id を潰した"
        );
        assert_eq!(
            reloaded.last_runtime_state,
            RuntimeState::Error,
            "last_runtime_state を潰した"
        );
        assert_eq!(
            reloaded.last_runtime_error.as_deref(),
            Some("last-runtime-error-value"),
            "last_runtime_error を潰した"
        );
        assert_eq!(
            reloaded.first_started_at,
            Some(111),
            "first_started_at を潰した"
        );
    }

    #[test]
    fn update_session_persists_all_seven_patchable_columns_and_bumps_updated_at_in_db() {
        // 既存テストは各カラムを個別に見ており、`description` の patch 適用経路
        // （if let Some(description) と SET description = ?2 の両方）や、
        // `sort_order` / `updated_at` が実際に DB に書かれたことは
        // どのテストからも検証されていなかった（戻り値は fetch_session 時点の
        // in-memory Session をそのまま返すため、戻り値だけを見るテストは
        // SET 句からの脱落を検出できない）。7 カラム（M3-3 の heuristics_enabled /
        // silence_timeout_secs を含む）を 1 回の patch で同時に当て、
        // 必ず get_session で DB から読み直して検証する。
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let session = Session {
            id: "sid-full-patch".to_owned(),
            project_id: pid,
            title: "before-title".to_owned(),
            description: "before-description".to_owned(),
            kanban_status: KanbanStatus::Backlog,
            sort_order: 1.0,
            mode: SessionMode::InPlace,
            branch: None,
            worktree_path: None,
            cli_kind: CliKind::Shell,
            cli_command: None,
            claude_session_id: None,
            last_runtime_state: RuntimeState::Idle,
            last_runtime_error: None,
            first_started_at: None,
            heuristics_enabled: false,
            silence_timeout_secs: 30,
            archived_at: None,
            created_at: 1,
            updated_at: 1,
        };
        store.insert_session(&session).expect("insert");

        store
            .update_session(
                &session.id,
                &patch_from_json(
                    r#"{"title":"after-title","description":"after-description",
                        "kanban_status":"review","sort_order":9.5,
                        "archived_at":1700000000000,
                        "heuristics_enabled":true,"silence_timeout_secs":45}"#,
                ),
            )
            .expect("update");

        let reloaded = store.get_session(&session.id).expect("get");
        assert_eq!(
            reloaded.title, "after-title",
            "title が DB に書かれていない"
        );
        assert_eq!(
            reloaded.description, "after-description",
            "description が DB に書かれていない"
        );
        assert_eq!(
            reloaded.kanban_status,
            KanbanStatus::Review,
            "kanban_status が DB に書かれていない"
        );
        assert_eq!(
            reloaded.sort_order, 9.5,
            "sort_order が DB に書かれていない"
        );
        assert_eq!(
            reloaded.archived_at,
            Some(1700000000000),
            "archived_at が DB に書かれていない"
        );
        assert!(
            reloaded.heuristics_enabled,
            "heuristics_enabled が DB に書かれていない"
        );
        assert_eq!(
            reloaded.silence_timeout_secs, 45,
            "silence_timeout_secs が DB に書かれていない"
        );
        assert!(
            reloaded.updated_at > session.updated_at,
            "updated_at が DB 上で進んでいない"
        );
    }

    /// 設定変更が UPDATE の SET 句に載っていないと、変更はメモリ上でしか効かず
    /// 再起動で元へ戻る（戻り値は patch 適用後の in-memory Session をそのまま返すので、
    /// 戻り値だけを見るテストでは検出できない）。必ず get_session で読み直す。
    #[test]
    fn heuristics_settings_survive_a_round_trip() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let session = Session::new_backlog(
            &pid,
            "a",
            "",
            SessionMode::InPlace,
            None,
            CliKind::Claude,
            None,
            1.0,
            now_ms(),
        );
        store.insert_session(&session).expect("insert");
        assert!(
            session.heuristics_enabled,
            "前提: claude の既定はオン（オフへの変更が既定値と区別できること）"
        );

        store
            .update_session(
                &session.id,
                &patch_from_json(r#"{"heuristics_enabled":false,"silence_timeout_secs":120}"#),
            )
            .expect("update");

        let reloaded = store.get_session(&session.id).expect("reload");
        assert!(!reloaded.heuristics_enabled, "オフ設定が永続化されていない");
        assert_eq!(
            reloaded.silence_timeout_secs, 120,
            "秒数が永続化されていない"
        );
    }

    /// 範囲外の `silence_timeout_secs` は `AppError::InvalidState` で拒否し、
    /// **同じ patch の他のフィールドも書かない**（検証が UPDATE より前にあること）。
    ///
    /// `update_session` は Tauri コマンドとして露出しており（`lib.rs`）、
    /// Rust 側のユーザー入力境界はここである。`clamp_timeout_secs` は黙って丸める
    /// 内部の保険で、拒否はしない（設計 §4.5）。
    #[test]
    fn update_session_rejects_an_out_of_range_silence_timeout_without_writing_the_row() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let session = Session {
            silence_timeout_secs: 90,
            ..session_with_sentinel_updated_at(&pid, "sid-timeout-rejected")
        };
        store.insert_session(&session).expect("insert");

        for out_of_range in [4_u32, 3601] {
            let err = store
                .update_session(
                    &session.id,
                    &patch_from_json(&format!(
                        r#"{{"title":"renamed","silence_timeout_secs":{out_of_range}}}"#
                    )),
                )
                .expect_err("範囲外は拒否するはず");
            match err {
                AppError::InvalidState(msg) => {
                    assert!(msg.contains("silence_timeout_secs"), "メッセージ: {msg}")
                }
                other => panic!("InvalidState を期待したが {other:?}"),
            }

            let reloaded = store.get_session(&session.id).expect("reload");
            assert_eq!(
                reloaded.silence_timeout_secs, 90,
                "拒否したのに秒数が書き換わっている"
            );
            assert_eq!(
                reloaded.title, session.title,
                "拒否したのに同じ patch の title が書かれている（検証が UPDATE より後にある）"
            );
        }
    }

    /// patch に無いフィールドは変更しない。`SET` 句が既定値を無条件に書いていると、
    /// 「タイトルを直しただけでヒューリスティック設定が既定へ戻る」という
    /// 見つけにくい退行になる。既定と区別できる値（shell なのに有効 / 30 でない秒数）
    /// を仕込まないと、この退行はどちらの列でも緑のまま通る。
    #[test]
    fn update_session_leaves_heuristics_settings_unchanged_when_the_patch_omits_them() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let session = Session {
            heuristics_enabled: true,
            silence_timeout_secs: 90,
            ..session_with_sentinel_updated_at(&pid, "sid-heuristics-untouched")
        };
        store.insert_session(&session).expect("insert");

        store
            .update_session(&session.id, &patch_from_json(r#"{"title":"after"}"#))
            .expect("update");

        let reloaded = store.get_session(&session.id).expect("get");
        assert!(
            reloaded.heuristics_enabled,
            "cli_kind の既定（shell = false）で上書きされた"
        );
        assert_eq!(
            reloaded.silence_timeout_secs, 90,
            "既定の 30 で上書きされた"
        );
    }

    /// `insert_session` は渡された `Session` をそのまま書く。ここで `cli_kind` から
    /// 既定値を再計算すると、`Session` が `false` を持っていても DB には `true` が入り、
    /// 構造体と DB が食い違う（split-brain）。既定値を決めるのは `Session::new_backlog`
    /// だけであり、その責務は `new_backlog_takes_the_heuristics_default_from_the_cli_kind`
    /// が固定している。
    #[test]
    fn insert_session_persists_the_sessions_own_heuristics_settings() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        // cli_kind から再計算すると true になる組み合わせを、値としては false で渡す
        let session = Session {
            cli_kind: CliKind::Claude,
            heuristics_enabled: false,
            silence_timeout_secs: 90,
            ..session_with_sentinel_updated_at(&pid, "sid-heuristics")
        };
        store.insert_session(&session).expect("insert");

        // row_to_session 側で辻褄が合わされていないことを見るため、生の列を読む
        let conn = store.conn().expect("conn");
        let (enabled, secs): (i64, i64) = conn
            .query_row(
                "SELECT heuristics_enabled, silence_timeout_secs FROM sessions WHERE id = ?1",
                [&session.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("row");
        assert_eq!(
            enabled, 0,
            "cli_kind から既定値を再計算して Session の値を上書きしている"
        );
        assert_eq!(secs, 90);
    }

    #[test]
    fn update_session_reports_not_found_for_unknown_id() {
        let (_dir, store) = open_temp();
        let err = store
            .update_session("nope", &patch_from_json(r#"{"title":"x"}"#))
            .expect_err("無い ID");
        match err {
            crate::error::AppError::NotFound(id) => assert_eq!(id, "nope"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn update_session_with_empty_patch_is_a_no_op_except_updated_at() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let s = insert_session_with_description(&store, &pid, "a", "d");

        std::thread::sleep(std::time::Duration::from_millis(3));
        let updated = store
            .update_session(&s.id, &patch_from_json("{}"))
            .expect("update");

        assert_eq!(updated.title, "a");
        assert_eq!(updated.description, "d");
        assert_eq!(updated.kanban_status, s.kanban_status);
        assert!(updated.updated_at > s.updated_at);
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
        // 20 フィールド全部に相互に区別できる非 NULL 値を入れる
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
            heuristics_enabled: false,
            silence_timeout_secs: 90,
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
        assert!(
            !fetched.heuristics_enabled,
            "cli_kind = custom の既定（true）で上書きされている"
        );
        assert_eq!(fetched.silence_timeout_secs, 90);
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
        //
        // id は UUID ではなく固定文字列で直接構築する（Session::new_backlog は
        // Uuid::new_v4() で埋めてしまい大小関係を制御できないため）。
        // 挿入順（rowid 順）と id の昇順が必ず逆になるよう、id が大きい行を先に、
        // 小さい行を後に挿入する。こうしないと UUID の偶然の大小関係次第で
        // 「挿入順 = id 昇順」になり、id ASC を消してもテストが green のままになりうる。
        let (_dir, store) = open_temp();
        let pid = project(&store);

        fn session_with_id(pid: &str, id: &str, sort_order: f64) -> Session {
            Session {
                id: id.to_owned(),
                project_id: pid.to_owned(),
                title: id.to_owned(),
                description: String::new(),
                kanban_status: KanbanStatus::Backlog,
                sort_order,
                mode: SessionMode::InPlace,
                branch: None,
                worktree_path: None,
                cli_kind: CliKind::Shell,
                cli_command: None,
                claude_session_id: None,
                last_runtime_state: RuntimeState::Idle,
                last_runtime_error: None,
                first_started_at: None,
                heuristics_enabled: true,
                silence_timeout_secs: 30,
                archived_at: None,
                created_at: 1,
                updated_at: 1,
            }
        }

        let zzz = session_with_id(&pid, "zzz-tie-break", 1.0);
        store.insert_session(&zzz).expect("insert zzz");
        let aaa = session_with_id(&pid, "aaa-tie-break", 1.0);
        store.insert_session(&aaa).expect("insert aaa");

        let list = store.list_sessions(&pid, false).expect("list");
        assert_eq!(
            list.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
            vec!["aaa-tie-break".to_owned(), "zzz-tie-break".to_owned()],
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

    #[test]
    fn set_worktree_persists_branch_and_path() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let s = session_with_sentinel_updated_at(&pid, "sid-worktree");
        store.insert_session(&s).expect("insert");

        store
            .set_worktree(
                &s.id,
                "session/fix-login",
                "/repo/.worktrees/session-fix-login",
            )
            .expect("set_worktree");

        let fetched = store.get_session(&s.id).expect("get");
        assert_eq!(fetched.branch.as_deref(), Some("session/fix-login"));
        assert_eq!(
            fetched.worktree_path.as_deref(),
            Some("/repo/.worktrees/session-fix-login")
        );
        // set_worktree はユーザー操作由来の変更なので updated_at を動かす（契約 §38.2）。
        // 1 は now_ms() と絶対に衝突しないセンチネル値
        assert!(
            fetched.updated_at > 1,
            "updated_at が動いていない（センチネル値 1 のまま）"
        );
    }

    #[test]
    fn set_worktree_path_to_null_keeps_branch() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let s = session_with_sentinel_updated_at(&pid, "sid-worktree-path");
        store.insert_session(&s).expect("insert");
        store
            .set_worktree(
                &s.id,
                "session/fix-login",
                "/repo/a/.worktrees/session-fix-login",
            )
            .expect("set_worktree");

        // `set_worktree` の時点でセンチネル値 1 は既に now_ms() に置き換わっている
        // （契約 §38.2: set_worktree はユーザー操作由来なので updated_at を動かす）。
        // `set_worktree_path` 自身が updated_at を動かすかどうかを固定するには、
        // このあと取得した値を基準にしないと恒真になる（レビュー Important 1）。
        let before = store.get_session(&s.id).expect("set_worktree 後の取得");

        // now_ms() はミリ秒精度なので、同一ミリ秒内の 2 呼び出しは
        // `>` 比較を偽陰性にしうる（`update_session_bumps_updated_at_but_not_created_at`
        // と同じ手当て）。
        std::thread::sleep(std::time::Duration::from_millis(3));

        store
            .set_worktree_path(&s.id, None)
            .expect("worktree_path のクリアに失敗");

        let reloaded = store.get_session(&s.id).expect("再取得に失敗");
        assert_eq!(
            reloaded.worktree_path, None,
            "worktree_path がクリアされていない"
        );
        assert_eq!(
            reloaded.branch.as_deref(),
            Some("session/fix-login"),
            "branch まで消えた。契約 §13 違反"
        );
        assert!(
            reloaded.updated_at > before.updated_at,
            "set_worktree_path が updated_at を動かしていない（契約 §38.2）"
        );
    }

    /// Ruling 15: brief 添付テストは 1 本のみで `changed == 0` のガード（4 番目の
    /// 変異対象）を通らない。存在しない id を渡す経路を独立に固定する。
    #[test]
    fn set_worktree_path_rejects_an_unknown_session_id() {
        let (_dir, store) = open_temp();

        let err = store
            .set_worktree_path("no-such-session", None)
            .expect_err("存在しない id が受理された");

        assert!(matches!(err, AppError::NotFound(_)), "想定外: {err:?}");
    }

    #[test]
    fn set_claude_session_id_persists_and_overwrites() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let s = session_with_sentinel_updated_at(&pid, "sid-claude");
        store.insert_session(&s).expect("insert");

        store.set_claude_session_id(&s.id, "cc-1").expect("first");
        let after_first = store.get_session(&s.id).expect("get");
        assert_eq!(after_first.claude_session_id.as_deref(), Some("cc-1"));
        // set_claude_session_id はユーザー操作由来の変更なので updated_at を動かす（契約 §38.2）
        assert!(
            after_first.updated_at > 1,
            "updated_at が動いていない（センチネル値 1 のまま）"
        );

        // --continue 経路では新しい ID で上書きになる（契約 §12.6）
        store.set_claude_session_id(&s.id, "cc-2").expect("second");
        assert_eq!(
            store
                .get_session(&s.id)
                .expect("get")
                .claude_session_id
                .as_deref(),
            Some("cc-2")
        );
    }

    #[test]
    fn setting_claude_session_id_for_unknown_session_is_not_found() {
        let (_dir, store) = open_temp();
        let err = store
            .set_claude_session_id("no-such-session", "x")
            .expect_err("must fail");
        assert!(
            matches!(err, AppError::NotFound(_)),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn set_last_runtime_state_persists_every_variant() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let s = session_with_sentinel_updated_at(&pid, "sid-state");
        store.insert_session(&s).expect("insert");

        for state in [
            RuntimeState::Running,
            RuntimeState::WaitingInput,
            RuntimeState::Exited,
            // DAO は interrupted を弾かない。不変条件を守るのは M2-1 の SessionManager
            RuntimeState::Interrupted,
            RuntimeState::Idle,
        ] {
            store.set_last_runtime_state(&s.id, state).expect("set");
            let after = store.get_session(&s.id).expect("get");
            assert_eq!(after.last_runtime_state, state);
            // runtime_state はシステム導出値。updated_at を動かさない（契約 §38.2）。
            // 1 は now_ms() と絶対に衝突しないセンチネル値
            assert_eq!(after.updated_at, 1, "{state:?} で updated_at が動いた");
        }
    }

    #[test]
    fn set_runtime_error_writes_state_and_message_together() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let s = session_with_sentinel_updated_at(&pid, "sid-error");
        store.insert_session(&s).expect("insert");

        store
            .set_runtime_error(&s.id, "claude: command not found\n")
            .expect("set_runtime_error");
        let after = store.get_session(&s.id).expect("get");
        assert_eq!(after.last_runtime_state, RuntimeState::Error);
        // 生 stderr をそのまま持つ。整形しない（契約 §4）
        assert_eq!(
            after.last_runtime_error.as_deref(),
            Some("claude: command not found\n")
        );
        // runtime_state 系は updated_at を動かさない（契約 §38.2）。
        // 1 は now_ms() と絶対に衝突しないセンチネル値
        assert_eq!(after.updated_at, 1);
    }

    #[test]
    fn set_last_runtime_state_clears_the_error_message_when_leaving_error() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let s = insert_test_session(&store, &pid, "a");

        store.set_runtime_error(&s.id, "boom").expect("error");
        store
            .set_last_runtime_state(&s.id, RuntimeState::Running)
            .expect("recover");

        // error 以外へ遷移したら last_runtime_error は NULL に戻る（契約 §17）。
        // 残すと、復帰したセッションに前回の stderr が貼り付いたままになる
        let after = store.get_session(&s.id).expect("get");
        assert_eq!(after.last_runtime_state, RuntimeState::Running);
        assert_eq!(after.last_runtime_error, None);
    }

    #[test]
    fn mark_first_started_writes_once_and_never_overwrites() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let s = session_with_sentinel_updated_at(&pid, "sid-first-started");
        store.insert_session(&s).expect("insert");
        assert_eq!(
            store.get_session(&s.id).expect("get").first_started_at,
            None
        );

        store.mark_first_started(&s.id, 1_000).expect("first");
        let after = store.get_session(&s.id).expect("get");
        assert_eq!(after.first_started_at, Some(1_000));
        // updated_at は動かさない（契約 §34.4）。
        // 1 は now_ms() と絶対に衝突しないセンチネル値
        assert_eq!(after.updated_at, 1);

        // 2 度目は 0 行更新で Ok。値は変わらない
        store.mark_first_started(&s.id, 2_000).expect("second");
        assert_eq!(
            store.get_session(&s.id).expect("get").first_started_at,
            Some(1_000)
        );

        // 不在の id も NotFound にしない（他のセッタと規約が違う）
        store.mark_first_started("nope", 3_000).expect("unknown id");
    }

    #[test]
    fn setters_report_not_found_for_unknown_id() {
        let (_dir, store) = open_temp();

        // conn.execute は行が無くても Ok(0) を返すので、影響行数を見ないと黙って成功する
        match store.set_worktree("nope", "b", "/p").expect_err("worktree") {
            crate::error::AppError::NotFound(id) => assert_eq!(id, "nope"),
            other => panic!("expected NotFound, got {other:?}"),
        }
        match store
            .set_claude_session_id("nope", "cc")
            .expect_err("claude id")
        {
            crate::error::AppError::NotFound(id) => assert_eq!(id, "nope"),
            other => panic!("expected NotFound, got {other:?}"),
        }
        match store
            .set_last_runtime_state("nope", RuntimeState::Running)
            .expect_err("runtime state")
        {
            crate::error::AppError::NotFound(id) => assert_eq!(id, "nope"),
            other => panic!("expected NotFound, got {other:?}"),
        }
        match store
            .set_runtime_error("nope", "boom")
            .expect_err("runtime error")
        {
            crate::error::AppError::NotFound(id) => assert_eq!(id, "nope"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn list_sessions_by_last_runtime_state_orders_by_created_at_then_id_and_spans_projects_ignoring_archived(
    ) {
        // ORDER BY created_at ASC, id ASC（契約 §17）を id の集合一致ではなく完全な
        // 並びで固定する。created_at をタイにした 2 件は、id 昇順と挿入順がわざと
        // 逆になるよう仕込む。こうしないと ORDER BY を削っても、あるいは id の
        // タイブレークを削っても green のまま通ってしまう。
        let (_dir, store) = open_temp();
        let p1 = project(&store);
        let p2 = store
            .insert_project("other", "/Users/x/repo/other", CliKind::Claude)
            .expect("p2")
            .id;

        fn session_with(pid: &str, id: &str, created_at: i64, state: RuntimeState) -> Session {
            Session {
                id: id.to_owned(),
                project_id: pid.to_owned(),
                title: id.to_owned(),
                description: String::new(),
                kanban_status: KanbanStatus::Backlog,
                sort_order: 1.0,
                mode: SessionMode::InPlace,
                branch: None,
                worktree_path: None,
                cli_kind: CliKind::Shell,
                cli_command: None,
                claude_session_id: None,
                last_runtime_state: state,
                last_runtime_error: None,
                first_started_at: None,
                heuristics_enabled: true,
                silence_timeout_secs: 30,
                archived_at: None,
                created_at,
                updated_at: created_at,
            }
        }

        // すべて Idle で挿入し、set_last_runtime_state で Running に昇格させる
        // （直接 Running で INSERT すると、セッタの WHERE id を丸ごと落として
        // 全行 UPDATE にする退行がこのテストをすり抜けてしまう。idle 行が
        // 巻き込まれずに残ることを検証する意味も兼ねる）。
        let early = session_with(&p1, "b-early", 100, RuntimeState::Idle);
        // created_at がタイの 2 件。挿入順は z → a だが、id ASC では a が先に来る
        let tie_z = session_with(&p2, "z-tie", 200, RuntimeState::Idle);
        let tie_a = session_with(&p1, "a-tie", 200, RuntimeState::Idle);
        let idle = session_with(&p1, "c-idle", 300, RuntimeState::Idle);

        store.insert_session(&tie_z).expect("insert z");
        store.insert_session(&tie_a).expect("insert a");
        store.insert_session(&early).expect("insert early");
        store.insert_session(&idle).expect("insert idle");

        store
            .set_last_runtime_state(&early.id, RuntimeState::Running)
            .expect("early -> running");
        store
            .set_last_runtime_state(&tie_z.id, RuntimeState::Running)
            .expect("tie_z -> running");
        store
            .set_last_runtime_state(&tie_a.id, RuntimeState::Running)
            .expect("tie_a -> running");
        // idle は意図的に Running へ上げない。無関係な行が巻き込まれていないことの検証

        // 起動時正規化はメンテナンス処理なのでアーカイブ済みも拾う
        {
            let conn = store.conn().expect("conn");
            conn.execute(
                "UPDATE sessions SET archived_at = 1 WHERE id = ?1",
                [&tie_z.id],
            )
            .expect("archive");
        }

        let running = store
            .list_sessions_by_last_runtime_state(RuntimeState::Running)
            .expect("list");
        assert_eq!(
            running.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
            vec![
                "b-early".to_owned(),
                "a-tie".to_owned(),
                "z-tie".to_owned(),
            ],
            "created_at ASC, id ASC の順で、プロジェクトを跨いでアーカイブ済みも含めて全件返っていない"
        );

        assert!(store
            .list_sessions_by_last_runtime_state(RuntimeState::WaitingInput)
            .expect("list")
            .is_empty());
    }

    #[test]
    fn update_session_does_not_clobber_setter_written_columns() {
        // update_session は 5 カラムしか書かない。将来「全カラム UPDATE」に
        // リファクタされると M1-4 と M2-2 の書き込みが消えるので、ここで固定する。
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let s = insert_test_session(&store, &pid, "a");

        store
            .set_worktree(&s.id, "session/a", "/repo/.worktrees/session-a")
            .expect("worktree");
        store
            .set_claude_session_id(&s.id, "cc-1")
            .expect("claude id");
        store
            .set_last_runtime_state(&s.id, RuntimeState::Running)
            .expect("state");

        store
            .update_session(&s.id, &patch_from_json(r#"{"title":"renamed"}"#))
            .expect("update");

        let fetched = store.get_session(&s.id).expect("get");
        assert_eq!(fetched.title, "renamed");
        assert_eq!(fetched.branch.as_deref(), Some("session/a"));
        assert_eq!(
            fetched.worktree_path.as_deref(),
            Some("/repo/.worktrees/session-a")
        );
        assert_eq!(fetched.claude_session_id.as_deref(), Some("cc-1"));
        assert_eq!(fetched.last_runtime_state, RuntimeState::Running);
    }

    #[test]
    fn setters_only_modify_the_targeted_session_row() {
        // 6 つのセッタ（set_worktree / set_claude_session_id / clear_claude_session_id /
        // set_last_runtime_state / set_runtime_error / mark_first_started）はどれも
        // `WHERE id = ?1` で対象行を絞っている。DB に 1 行しかないテストでは、この WHERE 句を
        // 丸ごと落として「全行 UPDATE」に退行しても、対象行と全行の結果が
        // 一致してしまい検出できない。2 行目（bystander）を用意し、
        // 各セッタを呼ぶたびに bystander が無変化のままであることを
        // get_session で読み直して確認する。
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let target = session_with_sentinel_updated_at(&pid, "sid-target");
        // bystander の claude_session_id は insert 時点で非 None の値を仕込む。
        // set_claude_session_id 経由で後から仕込むと、そのセッタ自身が bystander の
        // updated_at を動かしてしまい、下の update_session の sentinel 検査
        // （updated_at == 1）を壊す。
        let mut bystander = session_with_sentinel_updated_at(&pid, "sid-bystander");
        bystander.claude_session_id = Some("cc-bystander-sentinel".to_owned());
        store.insert_session(&target).expect("insert target");
        store.insert_session(&bystander).expect("insert bystander");

        store
            .set_worktree(&target.id, "session/target", "/repo/.worktrees/target")
            .expect("set_worktree");
        let after_worktree = store.get_session(&bystander.id).expect("get");
        assert_eq!(
            after_worktree.branch, None,
            "set_worktree が無関係な行の branch を書き換えた"
        );
        assert_eq!(
            after_worktree.worktree_path, None,
            "set_worktree が無関係な行の worktree_path を書き換えた"
        );
        assert_eq!(
            after_worktree.updated_at, 1,
            "set_worktree が無関係な行の updated_at を書き換えた"
        );

        store
            .set_claude_session_id(&target.id, "cc-target")
            .expect("set_claude_session_id");
        let after_claude = store.get_session(&bystander.id).expect("get");
        assert_eq!(
            after_claude.claude_session_id,
            Some("cc-bystander-sentinel".to_string()),
            "set_claude_session_id が無関係な行の claude_session_id を書き換えた"
        );

        // clear は「NULL 化」なので、bystander が事前に持つ非 None の値（insert 時に
        // 仕込んだ sentinel）が変化しないことを見て初めて WHERE 句の退行を検出できる。
        // None のまま比較すると None == None の恒真になる。
        store
            .clear_claude_session_id(&target.id)
            .expect("clear_claude_session_id");
        let after_clear = store.get_session(&bystander.id).expect("get");
        assert_eq!(
            after_clear.claude_session_id,
            Some("cc-bystander-sentinel".to_string()),
            "clear_claude_session_id が無関係な行の claude_session_id を NULL 化した"
        );
        // target 側は clear によって claude_session_id が None に戻っているはず。
        // 以降の target_after 検査（cc-target を期待）と整合させるため、実際の
        // 運用経路（--continue で再取得した ID を set し直す）に沿って書き戻す。
        store
            .set_claude_session_id(&target.id, "cc-target")
            .expect("set_claude_session_id restore after clear");

        store
            .set_last_runtime_state(&target.id, RuntimeState::Running)
            .expect("set_last_runtime_state");
        let after_state = store.get_session(&bystander.id).expect("get");
        assert_eq!(
            after_state.last_runtime_state,
            RuntimeState::Idle,
            "set_last_runtime_state が無関係な行の last_runtime_state を書き換えた"
        );

        store
            .set_runtime_error(&target.id, "boom")
            .expect("set_runtime_error");
        let after_error = store.get_session(&bystander.id).expect("get");
        assert_eq!(
            after_error.last_runtime_state,
            RuntimeState::Idle,
            "set_runtime_error が無関係な行の last_runtime_state を error に巻き込んだ"
        );
        assert_eq!(
            after_error.last_runtime_error, None,
            "set_runtime_error が無関係な行の last_runtime_error を書き換えた"
        );

        store
            .mark_first_started(&target.id, 999)
            .expect("mark_first_started");
        let after_first_started = store.get_session(&bystander.id).expect("get");
        assert_eq!(
            after_first_started.first_started_at, None,
            "mark_first_started が無関係な行の first_started_at を書き換えた"
        );

        // update_session (`WHERE id = ?7`) も同じ規律で検査する。7 本ある
        // update_session のテストはどれもセッション行が 1 件（または 0 件）の
        // DB で走っており、WHERE 句を丸ごと落として全行 UPDATE に退行しても
        // 全 green のまま通ってしまう（コミット 7c537c5 が他の 5 セッタに
        // 適用した bystander 検査が、この関数だけ抜けていた非対称の是正）。
        // bystander の初期値（title = "sid-bystander" / Backlog / 1.0 / None）は
        // ここで書く patch の値と全フィールドで区別できるようにしてある。
        store
            .update_session(
                &target.id,
                &patch_from_json(
                    r#"{"title":"target-updated-via-update_session",
                        "kanban_status":"in_progress","sort_order":99.0,
                        "archived_at":1700000000000}"#,
                ),
            )
            .expect("update_session");
        let after_update = store.get_session(&bystander.id).expect("get");
        assert_eq!(
            after_update.title, "sid-bystander",
            "update_session が無関係な行の title を書き換えた"
        );
        assert_eq!(
            after_update.kanban_status,
            KanbanStatus::Backlog,
            "update_session が無関係な行の kanban_status を書き換えた"
        );
        assert_eq!(
            after_update.sort_order, 1.0,
            "update_session が無関係な行の sort_order を書き換えた"
        );
        assert_eq!(
            after_update.archived_at, None,
            "update_session が無関係な行の archived_at を書き換えた"
        );
        assert_eq!(
            after_update.updated_at, 1,
            "update_session が無関係な行の updated_at を書き換えた"
        );

        // bystander が無変化なだけでなく、target 側が実際に正しく書けていることも確認する
        let target_after = store.get_session(&target.id).expect("get");
        assert_eq!(target_after.branch.as_deref(), Some("session/target"));
        assert_eq!(
            target_after.worktree_path.as_deref(),
            Some("/repo/.worktrees/target")
        );
        assert_eq!(target_after.claude_session_id.as_deref(), Some("cc-target"));
        assert_eq!(target_after.last_runtime_state, RuntimeState::Error);
        assert_eq!(target_after.last_runtime_error.as_deref(), Some("boom"));
        assert_eq!(target_after.first_started_at, Some(999));
        assert_eq!(
            target_after.title, "target-updated-via-update_session",
            "update_session が対象行の title を書いていない"
        );
        assert_eq!(target_after.kanban_status, KanbanStatus::InProgress);
        assert_eq!(target_after.sort_order, 99.0);
        assert_eq!(target_after.archived_at, Some(1700000000000));
    }

    #[test]
    fn move_session_into_empty_column_assigns_sort_order_one() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let target = insert_test_session(&store, &pid, "target");

        let column = store
            .move_session(&target.id, KanbanStatus::Review, 0)
            .expect("move_session");

        assert_eq!(column.len(), 1, "移動先の列だけが返る");
        assert_eq!(column[0].id, target.id);
        assert_eq!(column[0].sort_order, 1.0, "空の列への挿入は 1.0");
    }

    #[test]
    fn move_session_to_index_zero_places_before_the_first_row() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let existing = Session {
            sort_order: 5.0,
            kanban_status: KanbanStatus::Review,
            ..session_with_sentinel_updated_at(&pid, "existing")
        };
        store.insert_session(&existing).expect("insert existing");
        let target = insert_test_session(&store, &pid, "target");

        let column = store
            .move_session(&target.id, KanbanStatus::Review, 0)
            .expect("move_session");

        let moved = column
            .iter()
            .find(|s| s.id == target.id)
            .expect("moved row");
        assert_eq!(moved.sort_order, 4.0, "L[0] - 1.0");
    }

    #[test]
    fn move_session_to_index_at_end_places_after_the_last_row() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let existing = Session {
            sort_order: 5.0,
            kanban_status: KanbanStatus::Review,
            ..session_with_sentinel_updated_at(&pid, "existing")
        };
        store.insert_session(&existing).expect("insert existing");
        let target = insert_test_session(&store, &pid, "target");

        let column = store
            .move_session(&target.id, KanbanStatus::Review, 1)
            .expect("move_session");

        let moved = column
            .iter()
            .find(|s| s.id == target.id)
            .expect("moved row");
        assert_eq!(moved.sort_order, 6.0, "L[last] + 1.0");
    }

    #[test]
    fn move_session_to_a_middle_index_uses_the_midpoint_of_its_neighbors() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let low = Session {
            sort_order: 5.0,
            kanban_status: KanbanStatus::Review,
            ..session_with_sentinel_updated_at(&pid, "low")
        };
        let high = Session {
            sort_order: 10.0,
            kanban_status: KanbanStatus::Review,
            ..session_with_sentinel_updated_at(&pid, "high")
        };
        store.insert_session(&low).expect("insert low");
        store.insert_session(&high).expect("insert high");
        let target = insert_test_session(&store, &pid, "target");

        let column = store
            .move_session(&target.id, KanbanStatus::Review, 1)
            .expect("move_session");

        let moved = column
            .iter()
            .find(|s| s.id == target.id)
            .expect("moved row");
        assert_eq!(moved.sort_order, 7.5, "(5.0 + 10.0) / 2.0");
    }

    #[test]
    fn move_session_renumbers_the_whole_column_when_the_midpoint_is_exhausted() {
        // 契約 §49.3.1: 枯渇判定は厳密比較。振り直す対象は「target を挿入した後の
        // 移動先の列」全体（1.0, 2.0, 3.0, ...）である。L だけを 1..L.len() に振り直して
        // から中点を取り直す読み方は誤り（このケースだと中点が 1.5 になり、
        // 期待値の「移動した行は 2.0」と食い違う）。M1-2-kanban.md:222 の旧 TS 実装
        // （移動先の列を丸ごと 1.0, 2.0, 3.0, ... に振り直す）を Rust 側へ移した形。
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let low = Session {
            sort_order: 1.0,
            kanban_status: KanbanStatus::Review,
            ..session_with_sentinel_updated_at(&pid, "low")
        };
        let high = Session {
            // low の次に表現できる値。中点が丸めでどちらかに潰れる（決定的な枯渇ケース）
            sort_order: 1.0 + 2f64.powi(-52),
            kanban_status: KanbanStatus::Review,
            ..session_with_sentinel_updated_at(&pid, "high")
        };
        store.insert_session(&low).expect("insert low");
        store.insert_session(&high).expect("insert high");
        assert_ne!(
            store.get_session(&low.id).expect("get low").sort_order,
            store.get_session(&high.id).expect("get high").sort_order,
            "SQLite REAL の往復で 2 値が潰れていたら、このテストは枯渇を検証できていない"
        );
        let target = insert_test_session(&store, &pid, "target");

        let column = store
            .move_session(&target.id, KanbanStatus::Review, 1)
            .expect("move_session");

        let sort_orders: Vec<f64> = column.iter().map(|s| s.sort_order).collect();
        assert_eq!(
            sort_orders,
            vec![1.0, 2.0, 3.0],
            "列全体が 1.0 刻みに振り直される"
        );
        let moved = column
            .iter()
            .find(|s| s.id == target.id)
            .expect("moved row");
        assert_eq!(moved.sort_order, 2.0, "移動した行は挿入位置の整数を得る");
    }

    #[test]
    fn move_session_to_another_column_leaves_the_source_column_untouched() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let sibling = session_with_sentinel_updated_at(&pid, "sibling");
        store.insert_session(&sibling).expect("insert sibling");
        let target = Session {
            sort_order: 2.0,
            ..session_with_sentinel_updated_at(&pid, "target")
        };
        store.insert_session(&target).expect("insert target");

        let column = store
            .move_session(&target.id, KanbanStatus::Review, 0)
            .expect("move_session");

        assert_eq!(column.len(), 1, "移動先の列だけが返る");
        assert_eq!(column[0].id, target.id);
        assert!(
            column.iter().all(|s| s.id != sibling.id),
            "移動元の行が戻り値に混ざっている"
        );

        let sibling_after = store.get_session(&sibling.id).expect("get sibling");
        assert_eq!(
            sibling_after.sort_order, 1.0,
            "移動元の行の sort_order が動いた"
        );
        assert_eq!(
            sibling_after.updated_at, 1,
            "移動元の行の updated_at が動いた"
        );
    }

    #[test]
    fn move_session_within_the_same_column_can_move_a_card_earlier() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let a = Session {
            sort_order: 1.0,
            ..session_with_sentinel_updated_at(&pid, "a")
        };
        let b = Session {
            sort_order: 2.0,
            ..session_with_sentinel_updated_at(&pid, "b")
        };
        let c = Session {
            sort_order: 3.0,
            ..session_with_sentinel_updated_at(&pid, "c")
        };
        store.insert_session(&a).expect("insert a");
        store.insert_session(&b).expect("insert b");
        store.insert_session(&c).expect("insert c");

        // b を先頭へ。L（b を除く）= [a=1.0, c=3.0] なので to_index=0 → 0.0
        let column = store
            .move_session(&b.id, KanbanStatus::Backlog, 0)
            .expect("move_session");

        assert_eq!(
            column.len(),
            3,
            "同一列内の移動では移動先 = 移動元で 1 つの列が返る"
        );
        let ids: Vec<&str> = column.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["b", "a", "c"]);
        assert_eq!(column[0].sort_order, 0.0, "L[0] - 1.0");
    }

    #[test]
    fn move_session_within_the_same_column_can_move_a_card_later() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let a = Session {
            sort_order: 1.0,
            ..session_with_sentinel_updated_at(&pid, "a")
        };
        let b = Session {
            sort_order: 2.0,
            ..session_with_sentinel_updated_at(&pid, "b")
        };
        let c = Session {
            sort_order: 3.0,
            ..session_with_sentinel_updated_at(&pid, "c")
        };
        store.insert_session(&a).expect("insert a");
        store.insert_session(&b).expect("insert b");
        store.insert_session(&c).expect("insert c");

        // a を末尾へ。L（a を除く）= [b=2.0, c=3.0] なので to_index=2(=L.len()) → 4.0
        let column = store
            .move_session(&a.id, KanbanStatus::Backlog, 2)
            .expect("move_session");

        let ids: Vec<&str> = column.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["b", "c", "a"]);
        assert_eq!(column[2].sort_order, 4.0, "L[last] + 1.0");
    }

    #[test]
    fn move_session_clamps_to_index_beyond_the_column_length() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let existing = Session {
            sort_order: 5.0,
            kanban_status: KanbanStatus::Review,
            ..session_with_sentinel_updated_at(&pid, "existing")
        };
        store.insert_session(&existing).expect("insert existing");
        let target = insert_test_session(&store, &pid, "target");

        // L.len() == 1 なのに to_index = 99 を渡す。resolveDragEnd が「自分の列の
        // 背景」に落としたときに送る値を模す（契約 §49.3.2）
        let column = store
            .move_session(&target.id, KanbanStatus::Review, 99)
            .expect("move_session");

        let moved = column
            .iter()
            .find(|s| s.id == target.id)
            .expect("moved row");
        assert_eq!(
            moved.sort_order, 6.0,
            "L.len() にクランプされ、末尾（L[last]+1.0）になる"
        );
    }

    #[test]
    fn move_session_reports_not_found_for_unknown_id() {
        let (_dir, store) = open_temp();
        let err = store
            .move_session("nope", KanbanStatus::Review, 0)
            .expect_err("無い ID");
        match err {
            crate::error::AppError::NotFound(id) => assert_eq!(id, "nope"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn move_session_bumps_updated_at_only_for_the_moved_row() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let low = Session {
            sort_order: 1.0,
            kanban_status: KanbanStatus::Review,
            ..session_with_sentinel_updated_at(&pid, "low")
        };
        let high = Session {
            sort_order: 1.0 + 2f64.powi(-52),
            kanban_status: KanbanStatus::Review,
            ..session_with_sentinel_updated_at(&pid, "high")
        };
        store.insert_session(&low).expect("insert low");
        store.insert_session(&high).expect("insert high");
        let target = Session {
            sort_order: 1.0,
            ..session_with_sentinel_updated_at(&pid, "target")
        };
        store.insert_session(&target).expect("insert target");

        store
            .move_session(&target.id, KanbanStatus::Review, 1)
            .expect("move_session");

        let moved_after = store.get_session(&target.id).expect("get target");
        assert!(
            moved_after.updated_at > 1,
            "移動した行の updated_at が進んでいない"
        );

        let low_after = store.get_session(&low.id).expect("get low");
        let high_after = store.get_session(&high.id).expect("get high");
        assert_eq!(
            low_after.updated_at, 1,
            "再採番で sort_order だけ動いた行の updated_at が動いた"
        );
        assert_eq!(
            high_after.updated_at, 1,
            "再採番で sort_order だけ動いた行の updated_at が動いた"
        );
    }

    #[test]
    fn move_session_breaks_sort_order_ties_in_the_returned_column_by_id() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        // sort_order が同値の 2 行を仕込む。id は挿入順と昇順がわざと逆になるように
        // する（list_sessions_breaks_sort_order_ties_by_id と同じ作法）
        let zzz = Session {
            sort_order: 5.0,
            kanban_status: KanbanStatus::Review,
            ..session_with_sentinel_updated_at(&pid, "zzz-tie")
        };
        let aaa = Session {
            sort_order: 5.0,
            kanban_status: KanbanStatus::Review,
            ..session_with_sentinel_updated_at(&pid, "aaa-tie")
        };
        store.insert_session(&zzz).expect("insert zzz");
        store.insert_session(&aaa).expect("insert aaa");
        let target = insert_test_session(&store, &pid, "target");

        // L.len() == 2 の末尾へ挿入。mid = 5.0 + 1.0 = 6.0 で枯渇しない
        let column = store
            .move_session(&target.id, KanbanStatus::Review, 2)
            .expect("move_session");

        let ids: Vec<&str> = column.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["aaa-tie", "zzz-tie", target.id.as_str()]);
    }

    #[test]
    fn move_session_excludes_archived_rows_from_the_column_and_the_result() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let active = Session {
            sort_order: 5.0,
            kanban_status: KanbanStatus::Review,
            ..session_with_sentinel_updated_at(&pid, "active")
        };
        let archived = Session {
            sort_order: 3.0,
            kanban_status: KanbanStatus::Review,
            archived_at: Some(1),
            ..session_with_sentinel_updated_at(&pid, "archived")
        };
        store.insert_session(&active).expect("insert active");
        store.insert_session(&archived).expect("insert archived");
        let target = insert_test_session(&store, &pid, "target");

        let column = store
            .move_session(&target.id, KanbanStatus::Review, 0)
            .expect("move_session");

        assert_eq!(column.len(), 2, "アーカイブ済みは戻り値に含まれない");
        assert!(column.iter().all(|s| s.id != archived.id));
        let moved = column
            .iter()
            .find(|s| s.id == target.id)
            .expect("moved row");
        assert_eq!(
            moved.sort_order, 4.0,
            "L はアーカイブ済みを除くので active(5.0) の前 = 4.0。\
             archived(3.0) を数えていたら値がずれる"
        );
    }

    #[test]
    fn move_session_updates_kanban_status_to_the_destination_column() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let target = insert_test_session(&store, &pid, "target");

        let column = store
            .move_session(&target.id, KanbanStatus::Done, 0)
            .expect("move_session");

        assert_eq!(column[0].kanban_status, KanbanStatus::Done);
        let reloaded = store.get_session(&target.id).expect("get");
        assert_eq!(reloaded.kanban_status, KanbanStatus::Done);
    }

    #[test]
    fn move_session_scopes_the_column_to_the_moved_rows_project() {
        // カンバンの列はプロジェクト単位（next_sort_order / list_sessions と同じ絞り込み）。
        // これを落とすと他プロジェクトの行が L に混ざって to_index がずれ、
        // 戻り値が他プロジェクトの id で汚染される（§29.4 の is_scratch と同じ論法。導出）。
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let other_pid = store
            .insert_project("other", "/Users/x/repo/other", CliKind::Claude)
            .expect("other project")
            .id;

        let other_low = Session {
            sort_order: 1.0,
            kanban_status: KanbanStatus::Review,
            ..session_with_sentinel_updated_at(&other_pid, "other-low")
        };
        let other_high = Session {
            sort_order: 2.0,
            kanban_status: KanbanStatus::Review,
            ..session_with_sentinel_updated_at(&other_pid, "other-high")
        };
        store.insert_session(&other_low).expect("insert other-low");
        store
            .insert_session(&other_high)
            .expect("insert other-high");
        let target = insert_test_session(&store, &pid, "target");

        let column = store
            .move_session(&target.id, KanbanStatus::Review, 0)
            .expect("move_session");

        assert_eq!(
            column.len(),
            1,
            "他プロジェクトの行が L や戻り値に混ざっている"
        );
        assert_eq!(
            column[0].sort_order, 1.0,
            "他プロジェクトの sort_order を L に数えている（空の列として扱われていない）"
        );

        let other_low_after = store.get_session(&other_low.id).expect("get");
        let other_high_after = store.get_session(&other_high.id).expect("get");
        assert_eq!(
            other_low_after.sort_order, 1.0,
            "他プロジェクトの行の sort_order が動いた"
        );
        assert_eq!(
            other_high_after.sort_order, 2.0,
            "他プロジェクトの行の sort_order が動いた"
        );
    }
}

#[cfg(test)]
mod claude_session_id_tests {
    use crate::error::AppError;
    use crate::model::{CliKind, KanbanStatus, SessionMode};
    use crate::session::cli_args::{resume_plan, ResumePlan};
    use crate::store::test_support::{insert_test_session, open_temp};
    use crate::store::{now_ms, Store};

    const ID_A: &str = "550e8400-e29b-41d4-a716-446655440000";
    const ID_B: &str = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";

    fn project(store: &Store) -> String {
        store
            .insert_project("kamux", "/repo", CliKind::Claude)
            .expect("project")
            .id
    }

    /// 一時 DB にプロジェクト 1 件とセッション 1 件を作り、session_id を返す。
    /// TempDir を呼び出し側で束縛し続けさせるため、そのまま返す。
    fn fixture() -> (tempfile::TempDir, Store, String) {
        let (dir, store) = open_temp();
        let pid = project(&store);
        let session = insert_test_session(&store, &pid, "fix login");
        (dir, store, session.id)
    }

    #[test]
    fn newly_created_session_has_no_claude_session_id() {
        let (_dir, store, id) = fixture();
        let s = store.get_session(&id).expect("get");
        assert_eq!(s.claude_session_id, None);
    }

    /// --resume 経路では同じ ID が来る（契約 §12.6）。冪等であることを固定する。
    /// set_claude_session_id_persists_and_overwrites（本ファイル上部）は上書きのみ見ており、
    /// 同一 ID の再設定は別の分岐なので固定しておく。
    #[test]
    fn set_with_the_same_id_is_idempotent() {
        let (_dir, store, id) = fixture();
        store.set_claude_session_id(&id, ID_A).expect("set");
        store.set_claude_session_id(&id, ID_A).expect("set again");
        let s = store.get_session(&id).expect("get");
        assert_eq!(s.claude_session_id.as_deref(), Some(ID_A));
    }

    /// 第1部 §3.1: --continue フォールバックの自己修復。
    /// clear（無効 ID 破棄 or 未捕捉）→ --continue 起動 → 新 ID 書き戻し、で
    /// 次回は --resume 経路に戻れる状態になる。
    /// insert_test_session は CliKind::Shell 固定で resume_plan が FreshStart になってしまう
    /// ため、ここだけ Session::new_backlog を直接使って CliKind::Claude にする。
    #[test]
    fn continue_fallback_repairs_itself_after_write_back() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let sort_order = store
            .next_sort_order(&pid, KanbanStatus::Backlog)
            .expect("next_sort_order");
        let session = crate::model::Session::new_backlog(
            &pid,
            "fix login",
            "",
            SessionMode::Worktree,
            Some("session/fix-login".to_owned()),
            CliKind::Claude,
            None,
            sort_order,
            now_ms(),
        );
        let session = store.insert_session(&session).expect("insert");
        let id = session.id;

        store.clear_claude_session_id(&id).expect("clear");
        let cleared = store.get_session(&id).expect("get");
        assert_eq!(cleared.claude_session_id, None);
        // clear の目的そのもの: claude_session_id を消すことで、次回起動が
        // 誤って ClaudeResume（無効な ID を渡す）ではなく ClaudeContinue に落ちる。
        assert_eq!(resume_plan(&cleared), ResumePlan::ClaudeContinue);

        // --continue で起動した claude の SessionStart が新 ID を返す
        store.set_claude_session_id(&id, ID_B).expect("write back");

        let s = store.get_session(&id).expect("get");
        assert_eq!(s.claude_session_id.as_deref(), Some(ID_B));
        // 次回の再開は分岐表 行 1（ClaudeResume）に乗る
        assert_eq!(
            resume_plan(&s),
            ResumePlan::ClaudeResume {
                claude_session_id: ID_B.to_string()
            }
        );
    }

    /// updated_at をセンチネル値 1 で挿入し、set を経由せず直接 claude_session_id 付きで
    /// 作る（§38.2 と同じ手筋。兄弟セッタ set_claude_session_id_persists_and_overwrites を
    /// 参照）ことで、「clear 自身が updated_at を動かすか」を set の副作用と混同せず見る。
    #[test]
    fn clear_sets_it_back_to_null_and_moves_updated_at() {
        let (_dir, store) = open_temp();
        let pid = project(&store);
        let sort_order = store
            .next_sort_order(&pid, KanbanStatus::Backlog)
            .expect("next_sort_order");
        let mut session = crate::model::Session::new_backlog(
            &pid,
            "fix login",
            "",
            SessionMode::InPlace,
            None,
            CliKind::Shell,
            None,
            sort_order,
            1,
        );
        session.claude_session_id = Some(ID_A.to_owned());
        let session = store.insert_session(&session).expect("insert");
        let id = session.id;

        store.clear_claude_session_id(&id).expect("clear");
        let s = store.get_session(&id).expect("get");
        assert_eq!(s.claude_session_id, None);
        assert!(
            s.updated_at > 1,
            "updated_at が動いていない（センチネル値 1 のまま）"
        );
    }

    /// set 側の同種テスト（setting_claude_session_id_for_unknown_session_is_not_found、
    /// 本ファイル上部）はあるが、clear 側の不在チェックはどこにも無い。
    #[test]
    fn clear_on_missing_session_is_not_found() {
        let (_dir, store) = open_temp();
        let err = store
            .clear_claude_session_id("99999999-9999-4999-8999-999999999999")
            .expect_err("should fail");
        assert!(matches!(err, AppError::NotFound(_)), "got {err:?}");
    }
}
