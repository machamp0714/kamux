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
}
