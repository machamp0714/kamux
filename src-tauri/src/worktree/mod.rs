use std::path::Path;
use std::process::Command;

use crate::error::{AppError, AppResult};

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
