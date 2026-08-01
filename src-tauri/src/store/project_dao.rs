use rusqlite::{params, Row};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::model::{CliKind, Project};
use crate::store::{now_ms, Store};

pub(crate) const PROJECT_COLUMNS: &str = "id, name, repo_path, default_cli, created_at, updated_at";

fn row_to_project(row: &Row<'_>) -> AppResult<Project> {
    let default_cli: String = row.get("default_cli")?;
    Ok(Project {
        id: row.get("id")?,
        name: row.get("name")?,
        repo_path: row.get("repo_path")?,
        default_cli: CliKind::from_db_str(&default_cli)
            .ok_or_else(|| AppError::Db(format!("unknown default_cli in db: {default_cli}")))?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

impl Store {
    pub fn insert_project(
        &self,
        name: &str,
        repo_path: &str,
        default_cli: CliKind,
    ) -> AppResult<Project> {
        let conn = self.conn()?;
        let now = now_ms();
        let project = Project {
            id: Uuid::new_v4().to_string(),
            name: name.to_owned(),
            repo_path: repo_path.to_owned(),
            default_cli,
            created_at: now,
            updated_at: now,
        };

        conn.execute(
            "INSERT INTO projects (id, name, repo_path, default_cli, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                project.id,
                project.name,
                project.repo_path,
                project.default_cli.as_db_str(),
                project.created_at,
                project.updated_at,
            ],
        )?;

        Ok(project)
    }

    /// 契約 §17: 不在は Option ではなく AppError::NotFound で表す。
    /// Tauri コマンドとしては公開しない（Rust 内部専用。M2-3 の通知文言組立などが使う）。
    pub fn get_project(&self, id: &str) -> AppResult<Project> {
        let conn = self.conn()?;
        let sql = format!("SELECT {PROJECT_COLUMNS} FROM projects WHERE id = ?1");
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query_and_then(params![id], row_to_project)?;
        match rows.next() {
            Some(project) => project,
            None => Err(AppError::NotFound(id.to_owned())),
        }
    }

    pub fn list_projects(&self) -> AppResult<Vec<Project>> {
        let conn = self.conn()?;
        let sql = format!("SELECT {PROJECT_COLUMNS} FROM projects ORDER BY created_at ASC, id ASC");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_and_then([], row_to_project)?;
        rows.collect()
    }

    /// 契約 §44.2: 影響行数 0 は AppError::NotFound。冪等にしない
    /// （mark_first_started とは性質が違う）。sessions は契約 §3 の
    /// ON DELETE CASCADE で消えるので、アプリ側で子行を先に消さない。
    /// worktree と git ブランチには一切触れない（契約 §7.1）。
    pub fn delete_project(&self, id: &str) -> AppResult<()> {
        let conn = self.conn()?;
        let affected = conn.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
        if affected == 0 {
            return Err(AppError::NotFound(id.to_owned()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::model::CliKind;
    use crate::store::test_support::open_temp;

    #[test]
    fn insert_project_returns_row_with_uuid_and_timestamps() {
        let (_dir, store) = open_temp();
        let p = store
            .insert_project("kamux", "/Users/x/repo/kamux", CliKind::Claude)
            .expect("insert");

        assert_eq!(p.name, "kamux");
        assert_eq!(p.repo_path, "/Users/x/repo/kamux");
        assert_eq!(p.default_cli, CliKind::Claude);
        assert_eq!(p.id.len(), 36, "UUID v4 のハイフン付き文字列");
        assert!(p.created_at > 0);
        assert_eq!(p.created_at, p.updated_at, "作成時は両方同じ");
    }

    #[test]
    fn get_project_returns_the_row_or_not_found() {
        let (_dir, store) = open_temp();
        let created = store
            .insert_project("kamux", "/Users/x/repo/kamux", CliKind::Shell)
            .expect("insert");

        let fetched = store.get_project(&created.id).expect("get");
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.name, "kamux");
        assert_eq!(fetched.repo_path, "/Users/x/repo/kamux");
        assert_eq!(fetched.default_cli, CliKind::Shell, "列挙型が往復している");

        // 契約 §17: Option ではなく NotFound を返す
        let err = store.get_project("nope").expect_err("無い ID");
        match err {
            crate::error::AppError::NotFound(id) => assert_eq!(id, "nope"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn list_projects_returns_persisted_rows_in_creation_order() {
        let (_dir, store) = open_temp();
        assert!(store.list_projects().expect("list").is_empty());

        let a = store
            .insert_project("a", "/repo/a", CliKind::Claude)
            .expect("a");
        std::thread::sleep(std::time::Duration::from_millis(3));
        let b = store
            .insert_project("b", "/repo/b", CliKind::Shell)
            .expect("b");

        let list = store.list_projects().expect("list");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, a.id);
        assert_eq!(list[1].id, b.id);
        assert_eq!(list[1].default_cli, CliKind::Shell, "列挙型が往復している");
    }

    #[test]
    fn duplicate_repo_path_is_rejected_as_db_error() {
        let (_dir, store) = open_temp();
        store
            .insert_project("a", "/repo/same", CliKind::Claude)
            .expect("first");
        let err = store
            .insert_project("b", "/repo/same", CliKind::Claude)
            .expect_err("2 つ目は UNIQUE 制約で失敗するはず");

        match err {
            crate::error::AppError::Db(message) => {
                assert!(
                    message.to_lowercase().contains("unique"),
                    "生の SQLite メッセージ: {message}"
                );
            }
            other => panic!("expected AppError::Db, got {other:?}"),
        }
    }

    #[test]
    fn unknown_cli_kind_in_db_is_reported_as_db_error() {
        let (_dir, store) = open_temp();
        {
            let conn = store.conn().expect("conn");
            conn.execute(
                "INSERT INTO projects (id, name, repo_path, default_cli, created_at, updated_at)
                 VALUES ('p1', 'broken', '/repo/broken', 'gemini', 1, 1)",
                [],
            )
            .expect("insert");
        }
        let err = store
            .list_projects()
            .expect_err("未知の列挙値は握り潰さない");
        match err {
            crate::error::AppError::Db(message) => assert!(message.contains("gemini")),
            other => panic!("expected AppError::Db, got {other:?}"),
        }
    }

    // 契約 §44.2: 削除は CASCADE で sessions ごと消える。
    // 子行の挿入は Store::conn() の生 SQL で行う —— test_support::insert_test_session は
    // Task 8 の産物であり、ここで使うと Task 順の前方参照ができてしまう。
    #[test]
    fn delete_project_cascades_to_sessions() {
        let (_dir, store) = open_temp();
        let project = store
            .insert_project("kamux", "/Users/x/repo/kamux", CliKind::Claude)
            .expect("insert");

        {
            let conn = store.conn().expect("conn");
            conn.execute(
                "INSERT INTO sessions (id, project_id, title, sort_order, mode, cli_kind, created_at, updated_at)
                 VALUES ('s1', ?1, 'child', 1.0, 'in_place', 'shell', 1, 1)",
                [&project.id],
            )
            .expect("insert session");
        }

        store.delete_project(&project.id).expect("delete");

        let conn = store.conn().expect("conn");
        let projects: i64 = conn
            .query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))
            .expect("count projects");
        assert_eq!(projects, 0);
        let sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .expect("count sessions");
        assert_eq!(sessions, 0, "ON DELETE CASCADE で子行が消えていない");
    }

    // 契約 §44.2: 影響行数 0 は Ok(()) にしない（無言で成功しない）
    #[test]
    fn delete_project_reports_not_found_for_unknown_id() {
        let (_dir, store) = open_temp();
        let err = store.delete_project("nope").expect_err("無い ID");
        match err {
            crate::error::AppError::NotFound(id) => assert_eq!(id, "nope"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    // 契約 §44.2: delete_project は `WHERE id = ?1` で対象プロジェクトだけを消す。
    // delete_project_cascades_to_sessions はプロジェクト 1 件、
    // delete_project_reports_not_found_for_unknown_id は 0 件の DB で走るため、
    // WHERE 句を丸ごと落として `DELETE FROM projects` になっても両方 green のまま
    // 通ってしまう。bystander プロジェクト（子セッション付き）を用意し、
    // 削除後も生き残っていることを DB から数えて確認する。
    #[test]
    fn delete_project_leaves_other_projects_and_their_sessions_untouched() {
        let (_dir, store) = open_temp();
        let target = store
            .insert_project("target", "/repo/target", CliKind::Claude)
            .expect("insert target");
        let bystander = store
            .insert_project("bystander", "/repo/bystander", CliKind::Claude)
            .expect("insert bystander");

        {
            let conn = store.conn().expect("conn");
            conn.execute(
                "INSERT INTO sessions (id, project_id, title, sort_order, mode, cli_kind, created_at, updated_at)
                 VALUES ('s-target', ?1, 'target session', 1.0, 'in_place', 'shell', 1, 1)",
                [&target.id],
            )
            .expect("insert target session");
            conn.execute(
                "INSERT INTO sessions (id, project_id, title, sort_order, mode, cli_kind, created_at, updated_at)
                 VALUES ('s-bystander', ?1, 'bystander session', 1.0, 'in_place', 'shell', 1, 1)",
                [&bystander.id],
            )
            .expect("insert bystander session");
        }

        store.delete_project(&target.id).expect("delete target");

        {
            let conn = store.conn().expect("conn");
            let projects: i64 = conn
                .query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))
                .expect("count projects");
            assert_eq!(projects, 1, "無関係なプロジェクトまで消えた");

            let bystander_sessions: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sessions WHERE project_id = ?1",
                    [&bystander.id],
                    |r| r.get(0),
                )
                .expect("count bystander sessions");
            assert_eq!(
                bystander_sessions, 1,
                "無関係なプロジェクトの子セッションまで CASCADE で巻き込まれて消えた"
            );
        }

        let survivor = store
            .get_project(&bystander.id)
            .expect("bystander プロジェクトが消えている");
        assert_eq!(survivor.id, bystander.id);
    }

    // 契約 §17: list_projects_returns_persisted_rows_in_creation_order は
    // 2 件を挿入順どおりに期待しており、ORDER BY を全部落とした素のテーブル
    // スキャン（rowid 順）と区別が付かない。id は Uuid::new_v4() 由来のため、
    // ここでは Store::conn() の生 SQL で id / created_at を固定値にして挿入する。
    //
    // タイになる created_at の 2 行を、挿入順と id 昇順が逆転するように入れる。
    // 期待値はハードコードする（sort() で作ると、id ASC を落としても
    // 挿入順とたまたま一致してしまう可能性を排除できない）。
    #[test]
    fn list_projects_breaks_tied_created_at_by_id_ascending() {
        let (_dir, store) = open_temp();
        let conn = store.conn().expect("conn");
        // 先に大きい id (zzz)、後に小さい id (aaa) を挿入する
        conn.execute(
            "INSERT INTO projects (id, name, repo_path, default_cli, created_at, updated_at)
             VALUES ('zzz-tie', 'zzz', '/repo/zzz-tie', 'claude', 100, 100)",
            [],
        )
        .expect("insert zzz-tie");
        conn.execute(
            "INSERT INTO projects (id, name, repo_path, default_cli, created_at, updated_at)
             VALUES ('aaa-tie', 'aaa', '/repo/aaa-tie', 'claude', 100, 100)",
            [],
        )
        .expect("insert aaa-tie");
        drop(conn);

        let list = store.list_projects().expect("list");
        assert_eq!(
            list.iter().map(|p| p.id.clone()).collect::<Vec<_>>(),
            vec!["aaa-tie".to_owned(), "zzz-tie".to_owned()],
            "created_at 同値時に id ASC でタイブレークされていない"
        );
    }

    // created_at が異なる 2 行を、挿入順と created_at 昇順が逆転するように入れる。
    // id の大小関係もわざと created_at と逆にする（id = "aaa" を created_at が
    // 大きい行に、id = "zzz" を created_at が小さい行に割り当てる）ことで、
    // `ORDER BY id ASC` だけが残っても正しい並びと一致しないようにする。
    // こうしないと、id ASC 単独の結果が偶然正解と一致してテストが機能しない。
    #[test]
    fn list_projects_orders_by_created_at_ascending_before_id() {
        let (_dir, store) = open_temp();
        let conn = store.conn().expect("conn");
        // 先に created_at が新しい行 (id = aaa) を挿入する
        conn.execute(
            "INSERT INTO projects (id, name, repo_path, default_cli, created_at, updated_at)
             VALUES ('aaa-order', 'aaa', '/repo/aaa-order', 'claude', 200, 200)",
            [],
        )
        .expect("insert aaa-order");
        // 後に created_at が古い行 (id = zzz) を挿入する
        conn.execute(
            "INSERT INTO projects (id, name, repo_path, default_cli, created_at, updated_at)
             VALUES ('zzz-order', 'zzz', '/repo/zzz-order', 'claude', 100, 100)",
            [],
        )
        .expect("insert zzz-order");
        drop(conn);

        let list = store.list_projects().expect("list");
        assert_eq!(
            list.iter().map(|p| p.id.clone()).collect::<Vec<_>>(),
            vec!["zzz-order".to_owned(), "aaa-order".to_owned()],
            "created_at ASC が効いていない（id ASC だけでは正解と一致しない配置）"
        );
    }
}
