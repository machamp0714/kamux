use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{AppError, AppResult};

pub mod exclude;
pub mod slug;
pub mod suggest;

pub use exclude::{ensure_worktrees_excluded, git_common_dir, EXCLUDE_ENTRY};
pub use slug::title_slug;
pub use suggest::{
    branch_exists, branch_slug, suggest_branch_name, worktree_path_for, BRANCH_PREFIX,
};

// pub(crate) にするのは、Task 3/4/5（他モジュールの git テスト）からも
// `crate::worktree::test_support::TestRepo` を再利用できるようにするため
// （`store::test_support` と同じ可視性の前例に合わせた）。
#[cfg(test)]
pub(crate) mod test_support;

/// git CLI をサブプロセスで実行する。
///
/// 失敗時は **stderr を一切加工せず** `AppError::Git` に載せる（契約 §6 / 設計書 §12）。
/// ユーザーの git 設定（includeIf, credential.helper 等）を尊重するため、
/// ここでは環境変数を隔離しない。
pub fn run_git(cwd: &Path, args: &[&str]) -> AppResult<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0") // 対話プロンプトでハングさせない
        .output()
        .map_err(|e| AppError::Git(format!("failed to execute git: {e}")))?;

    if !output.status.success() {
        // 契約 §6: message には加工していない stderr をそのまま入れる。
        // trim 等の加工を挟まない（レビュー Important 1 の裁定: 契約が brief に優先する）。
        return Err(AppError::Git(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// worktree を作り、そのパスを返す（設計書 §10 / 契約 §13）。
///
/// `git worktree add {path} -b {branch}`（ベースは現在の HEAD）を **1 回だけ**実行する。
/// 失敗しても自動リトライも `--force` もしない（設計書 §12 / 設計判断 3）。
/// ブランチ名の妥当性・衝突は git 自身に判定させ、その結果（生の stderr）を
/// そのまま返す。ディレクトリ名側（入れ子・脱出防止）は `branch_slug` の責務であり、
/// ここでは検証しない。
pub fn create_worktree(repo_path: &Path, slug: &str, branch: &str) -> AppResult<PathBuf> {
    ensure_worktrees_excluded(repo_path)?;

    let path = worktree_path_for(repo_path, slug);
    let path_str = path
        .to_str()
        .ok_or_else(|| AppError::Git(format!("worktree path is not valid UTF-8: {path:?}")))?;

    run_git(repo_path, &["worktree", "add", path_str, "-b", branch])?;

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worktree::test_support::TestRepo;

    #[test]
    fn run_git_returns_stdout_on_success() {
        let repo = TestRepo::new();
        let out = run_git(repo.path(), &["rev-parse", "--abbrev-ref", "HEAD"]).expect("git ok");
        assert_eq!(out.trim(), "main");
    }

    #[test]
    fn run_git_returns_raw_stderr_as_git_error() {
        let repo = TestRepo::new();

        // run_git とは独立に同じコマンドを直接キャプチャし、生の stderr を
        // 基準値にする（devrunerification: レビュー Important 2）。
        // 装飾（trim/format 追記など）を混入させても弁別できるよう、
        // contains ではなく完全一致で比較する。
        let reference = Command::new("git")
            .args(["checkout", "no-such-branch"])
            .current_dir(repo.path())
            .output()
            .expect("spawn git directly for the reference capture");
        assert!(!reference.status.success(), "reference command must fail");
        let raw_stderr = String::from_utf8_lossy(&reference.stderr).to_string();

        let err = run_git(repo.path(), &["checkout", "no-such-branch"]).unwrap_err();
        match err {
            AppError::Git(msg) => {
                // 加工しない stderr がそのまま入っていること（契約 §6）。
                assert_eq!(
                    msg, raw_stderr,
                    "stderr should be passed through byte-for-byte verbatim"
                );
            }
            other => panic!("expected AppError::Git, got {other:?}"),
        }
    }

    #[test]
    fn creates_worktree_with_new_branch() {
        let repo = TestRepo::new();
        let path = create_worktree(repo.path(), "fix-login-bug", "session/fix-login-bug")
            .expect("create worktree");

        assert!(path.exists(), "worktree dir must exist: {path:?}");
        assert!(
            path.join("README.md").exists(),
            "HEAD content must be checked out"
        );

        // そのディレクトリが新ブランチを指していること
        let branch = run_git(&path, &["rev-parse", "--abbrev-ref", "HEAD"]).expect("rev-parse");
        assert_eq!(branch.trim(), "session/fix-login-bug");
    }

    #[test]
    fn create_worktree_registers_exclude_entry() {
        let repo = TestRepo::new();
        create_worktree(repo.path(), "fix-login-bug", "session/fix-login-bug").expect("create");

        let body = std::fs::read_to_string(repo.path().join(".git/info/exclude")).expect("read");
        assert!(
            body.lines().any(|l| l.trim() == EXCLUDE_ENTRY),
            "got:\n{body}"
        );
    }

    #[test]
    fn duplicate_branch_returns_raw_git_stderr_without_retrying() {
        let repo = TestRepo::new();
        repo.git(&["branch", "session/taken"]);

        let err = create_worktree(repo.path(), "taken", "session/taken").unwrap_err();

        // create_worktree とは独立に同じコマンドを直接キャプチャし、生の stderr を
        // 基準値にする（mod.rs の run_git_returns_raw_stderr_as_git_error と同じ手法）。
        // env は本番 run_git と揃える（GIT_TERMINAL_PROMPT=0 のみ、グローバル git
        // 設定は隔離しない）。path は create_worktree が計算するものと同一にする
        // （このエラーメッセージはパス非依存だが、念のため揃えておく）。
        let expected_path = worktree_path_for(repo.path(), "taken");
        let expected_path_str = expected_path.to_str().expect("utf8 path");
        let reference = Command::new("git")
            .args(["worktree", "add", expected_path_str, "-b", "session/taken"])
            .current_dir(repo.path())
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .expect("spawn git directly for the reference capture");
        assert!(!reference.status.success(), "reference command must fail");
        let raw_stderr = String::from_utf8_lossy(&reference.stderr).to_string();

        match err {
            AppError::Git(msg) => {
                // contains ではなく完全一致で比較する（Task 1 レビューで見つかった
                // 同型の穴: trim や format! での装飾混入を弁別するため）。
                assert_eq!(
                    msg, raw_stderr,
                    "raw git stderr expected, byte-for-byte, got: {msg}"
                );
            }
            other => panic!("expected AppError::Git, got {other:?}"),
        }

        // 自動リトライ・force をしていないこと（別名のディレクトリが増えていない）
        let entries: Vec<_> = std::fs::read_dir(repo.path().join(".worktrees"))
            .map(|d| d.filter_map(|e| e.ok()).collect())
            .unwrap_or_default();
        assert!(
            entries.is_empty(),
            "must not create any worktree on failure"
        );
    }

    #[test]
    fn creates_two_worktrees_side_by_side() {
        let repo = TestRepo::new();
        let a = create_worktree(repo.path(), "task-a", "session/task-a").expect("a");
        let b = create_worktree(repo.path(), "task-b", "session/task-b").expect("b");
        assert!(a.exists() && b.exists());
        assert_ne!(a, b);

        // exclude は重複追記されていない
        let body = std::fs::read_to_string(repo.path().join(".git/info/exclude")).expect("read");
        assert_eq!(
            body.lines().filter(|l| l.trim() == EXCLUDE_ENTRY).count(),
            1
        );
    }

    #[test]
    fn returned_path_canonicalizes_to_the_expected_worktree_location() {
        // tempfile::tempdir() は /var/folders/... を返すが /private/var/... への
        // シンボリックリンクなので、文字列一致ではなく canonicalize して比較する
        // （設計判断 13）。
        let repo = TestRepo::new();
        let path = create_worktree(repo.path(), "task-c", "session/task-c").expect("create");

        let expected = worktree_path_for(repo.path(), "task-c");
        assert_eq!(
            std::fs::canonicalize(&path).expect("canonicalize actual"),
            std::fs::canonicalize(&expected).expect("canonicalize expected"),
            "returned path must resolve to the deterministic worktree location"
        );
    }

    #[test]
    fn created_worktree_is_registered_with_the_main_repository() {
        // ディレクトリが出来ただけの状態と区別する。
        //
        // `.worktrees/x` は本体リポジトリの作業ツリーの内側にあるため、git は
        // 単なる `mkdir` で作っただけのディレクトリからでも上位を辿って本体の
        // .git を見つけてしまう。つまり `--git-common-dir` はどちらのケースでも
        // 本体の .git に解決され、弁別できない（実測で確認済み）。
        //
        // 弁別できるのは `--git-dir`: 登録済みの linked worktree では
        // `<main .git>/worktrees/task-d` を返す一方、単なるディレクトリでは
        // 本体の .git そのものを返す。
        let repo = TestRepo::new();
        let path = create_worktree(repo.path(), "task-d", "session/task-d").expect("create");

        let common_dir = git_common_dir(&path).expect("git common dir from inside worktree");
        assert_eq!(
            std::fs::canonicalize(&common_dir).expect("canonicalize common dir"),
            std::fs::canonicalize(repo.path().join(".git")).expect("canonicalize main .git"),
            "worktree must belong to the same repository as the main .git"
        );

        let git_dir = run_git(&path, &["rev-parse", "--git-dir"]).expect("git-dir");
        let git_dir = std::fs::canonicalize(git_dir.trim()).expect("canonicalize git-dir");
        let main_git_dir =
            std::fs::canonicalize(repo.path().join(".git")).expect("canonicalize main .git");
        assert_ne!(
            git_dir, main_git_dir,
            "a linked worktree's --git-dir must differ from the main .git \
             (a plain directory would report the main .git itself)"
        );
    }

    #[test]
    fn run_git_wraps_spawn_failure_when_cwd_does_not_exist() {
        // cwd が存在しないと Command::output() 自体が失敗する（spawn 失敗経路）。
        // これは git のコマンド失敗（非ゼロ終了・stderr あり）とは別の分岐であり、
        // どちらも `AppError::Git` に載るが到達するコードパスが異なる（mod.rs の
        // `.map_err` 側）。
        let err = run_git(
            Path::new("/nonexistent/path/for/run_git/probe"),
            &["status"],
        )
        .unwrap_err();
        match err {
            AppError::Git(msg) => {
                assert!(
                    msg.contains("failed to execute git"),
                    "spawn failure message should be wrapped with context, got: {msg}"
                );
            }
            other => panic!("expected AppError::Git, got {other:?}"),
        }
    }
}
