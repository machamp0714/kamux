use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{AppError, AppResult};
use crate::model::WorktreeStatus;

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

/// worktree に未コミットの変更（untracked を含む）があるかを調べる読み取り専用の操作。
/// 破壊操作は一切行わない（契約 §7.2）。
pub fn worktree_status(worktree_path: &Path) -> AppResult<WorktreeStatus> {
    let out = run_git(
        worktree_path,
        &["status", "--porcelain", "--untracked-files=normal"],
    )?;

    let entries: Vec<String> = out
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();

    Ok(WorktreeStatus {
        dirty: !entries.is_empty(),
        entries,
    })
}

/// worktree を削除する。契約 §13 に従い **ブランチは決して削除しない**。
/// `force == false` のとき、未コミット変更があれば git 自身が拒否し、その stderr が
/// そのまま `AppError::Git` に入る（契約 §6）。`run_git` 経由で実行するため、
/// 対話プロンプトでハングしない防御（`GIT_TERMINAL_PROMPT=0`）を共有する。
pub fn remove_worktree(repo_path: &Path, worktree_path: &Path, force: bool) -> AppResult<()> {
    let path_str = worktree_path.to_str().ok_or_else(|| {
        AppError::Git(format!(
            "worktree path is not valid UTF-8: {worktree_path:?}"
        ))
    })?;

    let mut args: Vec<&str> = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(path_str);

    run_git(repo_path, &args)?;

    Ok(())
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

    #[test]
    fn worktree_status_clean_worktree_is_not_dirty() {
        let repo = TestRepo::new();
        let wt = repo.add_worktree("session/clean");
        // add_worktree の doc コメントの断定（canonicalize 済み）を観測するアサーション。
        // レビュー Important 2: 断定だけで検算が無いと次のリファクタで消えるため。
        assert_eq!(
            wt,
            std::fs::canonicalize(&wt).expect("canonicalize actual"),
            "add_worktree の戻り値は canonicalize 済みであること"
        );

        let st = worktree_status(&wt).expect("worktree_status が失敗");

        assert!(
            !st.dirty,
            "変更のない worktree が dirty 判定された: {:?}",
            st.entries
        );
        assert!(
            st.entries.is_empty(),
            "entries が空でない: {:?}",
            st.entries
        );
    }

    #[test]
    fn worktree_status_untracked_file_only_is_dirty() {
        let repo = TestRepo::new();
        let wt = repo.add_worktree("session/untracked");
        std::fs::write(wt.join("new.txt"), "x\n").expect("書き込みに失敗");

        let st = worktree_status(&wt).expect("worktree_status が失敗");

        assert!(
            st.dirty,
            "untracked ファイルだけの worktree が clean 判定された"
        );
        assert_eq!(st.entries, vec!["?? new.txt".to_string()]);
    }

    #[test]
    fn worktree_status_modified_tracked_file_is_dirty() {
        let repo = TestRepo::new();
        let wt = repo.add_worktree("session/modified");
        std::fs::write(wt.join("README.md"), "# changed\n").expect("書き込みに失敗");

        let st = worktree_status(&wt).expect("worktree_status が失敗");

        assert!(
            st.dirty,
            "追跡ファイルを変更した worktree が clean 判定された"
        );
        assert_eq!(st.entries, vec![" M README.md".to_string()]);
    }

    #[test]
    fn worktree_status_missing_path_is_git_error() {
        let repo = TestRepo::new();

        let err = worktree_status(&repo.path().join(".worktrees/does-not-exist"))
            .expect_err("存在しないパスでエラーにならなかった");

        assert!(
            matches!(err, AppError::Git(_)),
            "想定外のエラー種別: {:?}",
            err
        );
    }

    #[test]
    fn remove_worktree_clean_succeeds_and_keeps_branch() {
        let repo = TestRepo::new();
        let wt = repo.add_worktree("session/clean-remove");

        remove_worktree(repo.path(), &wt, false).expect("clean な worktree の削除に失敗");

        assert!(
            !wt.exists(),
            "worktree ディレクトリが残っている: {}",
            wt.display()
        );
        assert!(
            branch_exists(repo.path(), "session/clean-remove"),
            "ブランチが削除された。契約 §13: ブランチは残さなければならない"
        );
    }

    /// 判定の一致テスト: アプリの dirty 判定と git の拒否は同じ条件でなければならない。
    /// untracked ファイル 1 個だけのケースで両方を同時に検証する。
    #[test]
    fn remove_worktree_untracked_only_is_refused_without_force() {
        let repo = TestRepo::new();
        let wt = repo.add_worktree("session/untracked-remove");
        std::fs::write(wt.join("new.txt"), "x\n").expect("書き込みに失敗");

        let st = worktree_status(&wt).expect("worktree_status が失敗");
        assert!(st.dirty, "アプリ側は clean と判定した");

        let err = remove_worktree(repo.path(), &wt, false)
            .expect_err("untracked ファイルがあるのに非 force 削除が成功した");
        assert!(
            matches!(err, AppError::Git(_)),
            "想定外のエラー種別: {:?}",
            err
        );
        assert!(wt.exists(), "拒否されたのに worktree が消えている");
    }

    #[test]
    fn remove_worktree_modified_is_refused_without_force() {
        let repo = TestRepo::new();
        let wt = repo.add_worktree("session/modified-remove");
        std::fs::write(wt.join("README.md"), "# changed\n").expect("書き込みに失敗");

        let err = remove_worktree(repo.path(), &wt, false)
            .expect_err("未コミット変更があるのに非 force 削除が成功した");

        assert!(
            matches!(err, AppError::Git(_)),
            "想定外のエラー種別: {:?}",
            err
        );
        assert!(wt.exists(), "拒否されたのに worktree が消えている");
    }

    #[test]
    fn remove_worktree_dirty_with_force_succeeds_and_keeps_branch() {
        let repo = TestRepo::new();
        let wt = repo.add_worktree("session/force-remove");
        std::fs::write(wt.join("README.md"), "# changed\n").expect("書き込みに失敗");
        std::fs::write(wt.join("new.txt"), "x\n").expect("書き込みに失敗");

        remove_worktree(repo.path(), &wt, true).expect("force 削除に失敗");

        assert!(!wt.exists(), "worktree ディレクトリが残っている");
        assert!(
            branch_exists(repo.path(), "session/force-remove"),
            "force 削除でブランチまで消えた。契約 §13 違反"
        );
    }

    #[test]
    fn remove_worktree_error_message_contains_raw_git_stderr() {
        let repo = TestRepo::new();
        let wt = repo.add_worktree("session/stderr-check");
        std::fs::write(wt.join("new.txt"), "x\n").expect("書き込みに失敗");

        let err = remove_worktree(repo.path(), &wt, false).expect_err("削除が成功してしまった");

        match err {
            AppError::Git(msg) => {
                // 契約 §6: 加工していない stderr がそのまま入る。
                // `contains("--force")` だけでは trim や prefix 付与を弁別できない
                // （レビュー Important 1）ため、行頭・行末も生の stderr と一致することを見る。
                // 実際の git stderr: "fatal: '<path>' contains modified or untracked
                // files, use --force to delete it\n"
                assert!(
                    msg.contains("--force"),
                    "git の stderr が加工されている可能性がある: {msg}"
                );
                assert!(
                    msg.starts_with("fatal:"),
                    "先頭に prefix が付与されている可能性がある: {msg:?}"
                );
                assert!(
                    msg.ends_with('\n'),
                    "末尾が trim されている可能性がある: {msg:?}"
                );
            }
            other => panic!("想定外のエラー種別: {other:?}"),
        }
    }

    /// `run_git` の第 1 引数（cwd）に渡すのは `repo_path` でなければならない。
    /// `worktree_path` と取り違えても、どちらも「パス」に見えて名前だけでは
    /// 判別できない（契約 §81.2 カテゴリ 3。レビュー Important 2）ため、
    /// 2 つの独立したリポジトリを用意して cwd の取り違えを弁別する。
    /// 正しい実装: repo_b を cwd にして repo_a の worktree を渡すと、git は
    /// 「is not a working tree」で拒否する（実測済み）。
    /// 取り違えた実装（cwd=worktree_path）: git は repo_a 自身の中で走るので、
    /// 対象の worktree はその配下の正当な worktree として削除に成功してしまう。
    #[test]
    fn remove_worktree_runs_git_in_repo_path_not_worktree_path() {
        let repo_a = TestRepo::new();
        let repo_b = TestRepo::new();
        let wt_a = repo_a.add_worktree("session/repo-a-wt");

        let err = remove_worktree(repo_b.path(), &wt_a, false)
            .expect_err("別リポジトリを cwd にしたのに削除が成功した（第 1 引数の取り違えの疑い）");

        match &err {
            AppError::Git(msg) => {
                // レビュー Important 3: matches!(err, AppError::Git(_)) だけでは
                // 「別の理由で無条件に Err を返す」ように実装が変わっても偽陽性で
                // 緑を維持してしまう（向きが逆のテスト）。期待している失敗理由
                // そのもの（実測済み: "fatal: '<path>' is not a working tree\n"）
                // を見て弁別する。
                assert!(
                    msg.contains("is not a working tree"),
                    "期待した失敗理由（cwd 取り違え）ではない可能性がある: {msg:?}"
                );
            }
            other => panic!("想定外のエラー種別: {other:?}"),
        }
        assert!(
            wt_a.exists(),
            "取り違えにより repo_a の worktree が削除された"
        );
    }

    #[test]
    fn worktree_status_non_git_directory_is_git_error() {
        // レビュー Important 1: 契約 §6 の主経路（git は起動したが非ゼロ終了 -> 生 stderr）を
        // worktree_status_missing_path_is_git_error（spawn 失敗経路のみ）は通っていなかった。
        // TestRepo を使わず、git init していないただの一時ディレクトリを渡す。
        let dir = tempfile::TempDir::new().expect("tempdir 作成に失敗");

        let err = worktree_status(dir.path())
            .expect_err("git リポジトリでないディレクトリでエラーにならなかった");

        match err {
            AppError::Git(msg) => {
                assert!(
                    msg.contains("not a git repository"),
                    "非ゼロ終了時の生 stderr がそのまま入っていない: {msg}"
                );
            }
            other => panic!("expected AppError::Git, got {other:?}"),
        }
    }
}
