//! テスト専用の hermetic な git リポジトリ生成ヘルパ。
//!
//! 開発者のグローバル git 設定（commit.gpgsign, init.defaultBranch 等）に
//! テスト結果が左右されないよう、リポジトリ準備時のみ設定を隔離する。
//! 本番の `run_git` は隔離しない（ユーザーの git 設定を尊重するため）。
//!
//! ゲーティングは呼び出し側の `#[cfg(test)] pub(crate) mod test_support;`
//! （worktree/mod.rs）で行う。ここで重ねて `#![cfg(test)]` を書くと
//! clippy::duplicated_attributes に引っかかるため書かない。

use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

pub struct TestRepo {
    dir: TempDir,
}

impl TestRepo {
    /// main ブランチ・初期コミット 1 個の git リポジトリを作る。
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("create tempdir");
        let repo = TestRepo { dir };
        repo.git(&["init", "-b", "main", "."]);
        repo.git(&["config", "user.email", "test@example.com"]);
        repo.git(&["config", "user.name", "kamux test"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.path().join("README.md"), "hi\n").expect("write README");
        repo.git(&["add", "README.md"]);
        repo.git(&["commit", "-m", "init"]);
        repo
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// 隔離した環境で git を実行する。失敗したら panic（テストの前提条件なので）。
    pub fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(self.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    }
}
