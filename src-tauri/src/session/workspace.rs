//! セッションの作業ツリー（cwd）を用意し、起動成功時のカンバン遷移を適用する。
//!
//! `mode == in_place` では git 操作を一切行わない（契約 §13）。

use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};
use crate::model::{KanbanStatus, Project, Session, SessionMode};
use crate::worktree::{branch_slug, create_worktree, suggest_branch_name};

/// セッションの cwd を返す。worktree モードでは必要なら worktree を作り、
/// `session.branch` / `session.worktree_path` を埋める（呼び出し側が DB へ保存する）。
///
/// 分岐（設計判断 8）:
/// - `worktree_path` が `NULL` → 新規に `suggest_branch_name` → `create_worktree`
/// - `worktree_path` があり、ディレクトリが存在する → **再利用**。git 操作は一切しない
/// - `worktree_path` があり、ディレクトリが消えている → `AppError::Git` で報告し、
///   **黙って再作成しない**（掃除済み・手動削除・ディスク障害のいずれもありうるため。
///   自動復旧するとユーザーが意図的に消した worktree が復活してしまう。掃除 UX は M3-4）
pub fn prepare_worktree(project: &Project, session: &mut Session) -> AppResult<PathBuf> {
    if session.mode == SessionMode::InPlace {
        return Ok(PathBuf::from(&project.repo_path));
    }

    let repo = Path::new(&project.repo_path);

    // 既に worktree を持っている場合
    if let Some(existing) = session.worktree_path.clone() {
        let existing_path = PathBuf::from(&existing);
        if existing_path.is_dir() {
            return Ok(existing_path); // 再利用。git は叩かない
        }
        return Err(AppError::Git(format!(
            "worktree ディレクトリが見つかりません: {existing}\n\
             ブランチ {branch} は残っています。\n\
             掃除してから作り直すか、ディレクトリを復元してください。",
            branch = session.branch.as_deref().unwrap_or("(不明)")
        )));
    }

    // ユーザーが編集したブランチ名があればそれを使う。無ければ提案を採る。
    let branch = match session.branch.clone() {
        Some(b) => b,
        None => suggest_branch_name(repo, &session.title, &session.id)?,
    };
    // ディレクトリ名は branch_slug で導く（契約 §60.2.2）。
    // `branch` はユーザーが手で編集できる入力値であり §13 適合は保証されない（§51.3.2）ので、
    // `strip_prefix` だけでは `feature/foo` が `.worktrees/feature/foo` という入れ子を、
    // `../x` がリポジトリ外への脱出を作る。**接頭辞を剥がすだけにしないこと。**
    let slug = branch_slug(&branch, &session.id);

    let path = create_worktree(repo, &slug, &branch)?;
    let cwd = path
        .to_str()
        .ok_or_else(|| AppError::Git(format!("worktree path is not valid UTF-8: {path:?}")))?
        .to_string();

    session.branch = Some(branch);
    session.worktree_path = Some(cwd);
    Ok(path)
}

/// PTY 起動が成功した後のカンバン列の遷移（設計判断 6）。
///
/// 設計書 §5.3 の「自動遷移は backlog → in_progress のみ」を守る。
/// review / done / in_progress のセッションは列を動かさない
/// （`review` 列のセッションを再起動したときに引き戻すと、ユーザーのワークフロー
/// 状態を破壊するため）。
///
/// **`last_runtime_state` は触らない。** runtime_state の導出と永続化は
/// M2-1 が `pty://exit` 購読で一元的に行う（設計判断 7）。M1-4 が書いてよいのは
/// `kanban_status` / `sort_order` / `branch` / `worktree_path` だけである。
pub fn apply_start_kanban_transition(session: &mut Session, in_progress_sort_order: f64) {
    if session.kanban_status == KanbanStatus::Backlog {
        session.kanban_status = KanbanStatus::InProgress;
        session.sort_order = in_progress_sort_order;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CliKind, KanbanStatus, RuntimeState};
    use crate::worktree::test_support::TestRepo;

    fn project_at(repo_path: &std::path::Path) -> Project {
        Project {
            id: "proj-1".to_string(),
            name: "demo".to_string(),
            repo_path: repo_path.to_str().expect("utf8").to_string(),
            default_cli: CliKind::Claude,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn session_with(mode: SessionMode, title: &str) -> Session {
        Session {
            id: "3f2a1b9c-4d5e-6f70-8192-a3b4c5d6e7f8".to_string(),
            project_id: "proj-1".to_string(),
            title: title.to_string(),
            description: String::new(),
            kanban_status: KanbanStatus::Backlog,
            sort_order: 1.0,
            mode,
            branch: None,
            worktree_path: None,
            cli_kind: CliKind::Claude,
            cli_command: None,
            claude_session_id: None,
            last_runtime_state: RuntimeState::Idle,
            last_runtime_error: None,
            first_started_at: None,
            archived_at: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn in_place_mode_uses_repo_path_and_touches_no_git() {
        let repo = TestRepo::new();
        let project = project_at(repo.path());
        let mut session = session_with(SessionMode::InPlace, "Fix login bug");

        let cwd = prepare_worktree(&project, &mut session).expect("prepare");

        assert_eq!(cwd, std::path::PathBuf::from(&project.repo_path));
        assert_eq!(session.branch, None, "in_place では branch は NULL のまま");
        assert_eq!(session.worktree_path, None);
        assert!(
            !repo.path().join(".worktrees").exists(),
            "in_place では .worktrees を作らない"
        );
        assert!(
            !repo.path().join(".git/info/exclude").exists()
                || !std::fs::read_to_string(repo.path().join(".git/info/exclude"))
                    .unwrap_or_default()
                    .contains(".worktrees/"),
            "in_place では exclude も触らない"
        );
    }

    /// 通し経路（合成）テスト 1/2: `suggest_branch_name` → `branch_slug` →
    /// `worktree_path_for` → `create_worktree` を実際に繋いだときに、生成される
    /// ディレクトリの basename が `branch_slug(branch, session.id)` の出力と
    /// 一致することを確かめる。各リンクは個別に検証済みでも、合成が正しいかは
    /// これまで誰も見ていなかった（PR 11 レビューの申し送り）。
    #[test]
    fn worktree_mode_creates_and_records_branch_and_path() {
        let repo = TestRepo::new();
        let project = project_at(repo.path());
        let mut session = session_with(SessionMode::Worktree, "Fix login bug");

        let cwd = prepare_worktree(&project, &mut session).expect("prepare");

        let branch = session.branch.clone().expect("branch must be set");
        assert_eq!(branch, "session/fix-login-bug");
        assert_eq!(
            session.worktree_path.as_deref(),
            Some(cwd.to_str().expect("utf8"))
        );
        assert!(cwd.join("README.md").exists());

        // 合成の弁別点: 作られたディレクトリの basename は branch_slug(branch, id)
        // そのものであること。ハードコードした文字列ではなく本番と同じ導出関数を
        // 呼んで比較するので、branch_slug と create_worktree の間で slug がずれる
        // 変異を弁別できる。
        let expected_slug = branch_slug(&branch, &session.id);
        assert_eq!(
            cwd.file_name().and_then(|n| n.to_str()),
            Some(expected_slug.as_str())
        );
    }

    #[test]
    fn worktree_mode_honours_user_edited_branch_name() {
        let repo = TestRepo::new();
        let project = project_at(repo.path());
        let mut session = session_with(SessionMode::Worktree, "Fix login bug");
        session.branch = Some("session/my-own-name".to_string());

        prepare_worktree(&project, &mut session).expect("prepare");

        assert_eq!(session.branch.as_deref(), Some("session/my-own-name"));
        assert!(repo.path().join(".worktrees/my-own-name").exists());
    }

    /// 通し経路（合成）テスト 2/2: ユーザーが `session/` を付けずにスラッシュを
    /// 含むブランチ名を手で入力した場合。`branch.strip_prefix(BRANCH_PREFIX)
    /// .unwrap_or(&branch)` のような「剥がすだけ」の実装だと、スラッシュがその
    /// ままディレクトリ名に流れ込んで `.worktrees/feature/foo` という入れ子が
    /// できてしまう（契約 §60.2.2 が名指しで禁じている変異）。`branch_slug` を
    /// 通していれば `.worktrees/feature-foo` にフラット化される。
    #[test]
    fn worktree_mode_flattens_a_user_edited_branch_with_slashes() {
        let repo = TestRepo::new();
        let project = project_at(repo.path());
        let mut session = session_with(SessionMode::Worktree, "Fix login bug");
        session.branch = Some("feature/foo".to_string());

        let cwd = prepare_worktree(&project, &mut session).expect("prepare");

        assert_eq!(cwd, repo.path().join(".worktrees/feature-foo"));
        assert!(
            repo.path().join(".worktrees/feature-foo").is_dir(),
            "flat directory must exist"
        );
        assert!(
            !repo.path().join(".worktrees/feature").exists(),
            "must not create a nested .worktrees/feature directory"
        );
    }

    /// 弁別力: 再利用分岐を落とす（`if let Some(existing) = ...` を消す、または
    /// 常に新規作成する）変異を入れると、2 回目の呼び出しが同じブランチ名で
    /// `create_worktree` を再度叩き、git が「branch already exists」で失敗して
    /// `.expect("second")` が panic する。`assert_eq!(first, second)` 自体は弱く
    /// 見えるが、そこに到達できること自体が再利用分岐が働いた証拠になる。
    #[test]
    fn reuses_existing_worktree_without_running_git() {
        let repo = TestRepo::new();
        let project = project_at(repo.path());
        let mut session = session_with(SessionMode::Worktree, "Fix login bug");

        let first = prepare_worktree(&project, &mut session).expect("first");
        // 2 回目は既存を再利用し、ブランチ重複エラーにならないこと
        let second = prepare_worktree(&project, &mut session).expect("second");

        assert_eq!(first, second);
    }

    #[test]
    fn missing_worktree_dir_errors_instead_of_recreating() {
        let repo = TestRepo::new();
        let project = project_at(repo.path());
        let mut session = session_with(SessionMode::Worktree, "Fix login bug");

        let cwd = prepare_worktree(&project, &mut session).expect("first");
        std::fs::remove_dir_all(&cwd).expect("remove worktree dir");

        let err = prepare_worktree(&project, &mut session).unwrap_err();
        match err {
            AppError::Git(msg) => {
                assert!(
                    msg.contains("session/fix-login-bug"),
                    "must name branch: {msg}"
                );
                assert!(
                    msg.contains(cwd.to_str().expect("utf8")),
                    "must name the missing path: {msg}"
                );
            }
            other => panic!("expected AppError::Git, got {other:?}"),
        }
        assert!(!cwd.exists(), "must not silently recreate the worktree");
    }

    /// `existing_path.is_dir()` を `.exists()` に緩める変異を弁別する。通常
    /// ファイルは `exists()` を通ってしまうため、そのまま cwd として返すと
    /// portable-pty が黙って `$HOME` にフォールバックする実害がある
    /// （`session/mod.rs` の `plan_agent_spawn` が同種の理由で `is_dir()` を
    /// 使っているのと同じ手当て）。
    #[test]
    fn existing_worktree_path_that_is_a_regular_file_errors() {
        let repo = TestRepo::new();
        let project = project_at(repo.path());
        let mut session = session_with(SessionMode::Worktree, "Fix login bug");
        let fake_path = repo.path().join("not-a-directory");
        std::fs::write(&fake_path, b"").expect("write regular file");
        session.branch = Some("session/fix-login-bug".to_string());
        session.worktree_path = Some(fake_path.to_str().expect("utf8").to_string());

        let err = prepare_worktree(&project, &mut session).unwrap_err();

        assert!(matches!(err, AppError::Git(_)), "actual: {err:?}");
    }

    fn session_in(status: KanbanStatus) -> Session {
        let mut s = session_with(SessionMode::Worktree, "Fix login bug");
        s.id = "sess-1".to_string();
        s.kanban_status = status;
        s
    }

    #[test]
    fn backlog_moves_to_in_progress_at_column_tail() {
        let mut s = session_in(KanbanStatus::Backlog);
        apply_start_kanban_transition(&mut s, 7.0);
        assert_eq!(s.kanban_status, KanbanStatus::InProgress);
        assert_eq!(s.sort_order, 7.0, "In Progress 列の末尾に置く");
    }

    #[test]
    fn review_column_is_not_dragged_back_to_in_progress() {
        // 設計書 §5.3: 自動遷移は backlog → in_progress のみ
        let mut s = session_in(KanbanStatus::Review);
        s.sort_order = 3.0;
        apply_start_kanban_transition(&mut s, 7.0);
        assert_eq!(s.kanban_status, KanbanStatus::Review);
        assert_eq!(
            s.sort_order, 3.0,
            "列を動かさないなら sort_order も動かさない"
        );
    }

    #[test]
    fn in_progress_stays_put_on_restart() {
        let mut s = session_in(KanbanStatus::InProgress);
        s.sort_order = 2.0;
        apply_start_kanban_transition(&mut s, 7.0);
        assert_eq!(s.kanban_status, KanbanStatus::InProgress);
        assert_eq!(s.sort_order, 2.0);
    }

    #[test]
    fn done_column_stays_put_on_restart() {
        let mut s = session_in(KanbanStatus::Done);
        apply_start_kanban_transition(&mut s, 7.0);
        assert_eq!(s.kanban_status, KanbanStatus::Done);
    }

    #[test]
    fn start_transition_never_touches_runtime_state() {
        // 判断 7 / 契約 §2: runtime_state を書くのは M2-1 だけ
        let mut s = session_in(KanbanStatus::Backlog);
        s.last_runtime_state = RuntimeState::Idle;
        apply_start_kanban_transition(&mut s, 7.0);
        assert_eq!(
            s.last_runtime_state,
            RuntimeState::Idle,
            "M1-4 は last_runtime_state を書いてはならない"
        );
    }
}
