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
        return Err(AppError::Git(
            String::from_utf8_lossy(&output.stderr)
                .trim_end()
                .to_string(),
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
        let err = run_git(repo.path(), &["checkout", "no-such-branch"]).unwrap_err();
        match err {
            AppError::Git(msg) => {
                // 加工しない stderr がそのまま入っていること（契約 §6）
                assert!(
                    msg.contains("no-such-branch"),
                    "stderr should be passed through verbatim, got: {msg}"
                );
            }
            other => panic!("expected AppError::Git, got {other:?}"),
        }
    }
}
