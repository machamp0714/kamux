use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};
use crate::model::{CliKind, Session};
use crate::pty::launch_env::LaunchEnv;

/// PTY で起動するプログラム・引数・作業ディレクトリ・追加環境変数（契約 §23）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
}

/// 再開モード（契約 §23）。M2-4 が消費する
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeMode<'a> {
    /// 新規起動
    None,
    /// claude --continue
    Continue,
    /// claude --resume <id>
    SessionId(&'a str),
}

/// ユーザーのログインシェル。未設定なら macOS 既定の /bin/zsh。
/// §18 の `probe_login_env()`（PATH 探査）とは別物で、`cli_kind == Shell` 専用。
pub fn login_shell() -> String {
    login_shell_from(std::env::var("SHELL").ok())
}

/// `login_shell()` の判定ロジックを純関数に切り出したもの。`$SHELL` を直接読まないので
/// 環境変数の書き換えなしにガード節の分岐をテストできる（並列テストとの干渉を避けるため）。
fn login_shell_from(shell: Option<String>) -> String {
    match shell {
        Some(shell) if shell.starts_with('/') => shell,
        _ => "/bin/zsh".to_string(),
    }
}

/// 起動コマンドを組み立てる**純粋関数**（契約 §23）。
///
/// この関数の中で PATH 解決・DB 読み・プロセス起動を一切行わない。
/// `program` は §18 の `resolve_program()` が解決した絶対パス、
/// `cwd` は呼び出し側が `resolve_cwd()` / worktree 準備で決めた作業ディレクトリ、
/// `launch_env` は §18 の探査結果（PATH / LANG）。
///
/// M1-3 は `shell` の腕のみ実装する。`claude` / `codex` / `custom` は M1-4 が
/// この `match` に腕を足す（関数名・戻り型・引数は変えない）。
pub fn build_launch_command(
    session: &Session,
    program: &str,
    cwd: &Path,
    launch_env: &LaunchEnv,
    resume: ResumeMode<'_>,
) -> AppResult<LaunchCommand> {
    // M2-2 の kamux-relay が自セッションを識別するための環境変数。全 cli_kind 共通（§23）
    let env = vec![("KAMUX_SESSION_ID".to_string(), session.id.clone())];

    match session.cli_kind {
        CliKind::Shell => {
            // PTY に繋がった $SHELL -l はインタラクティブシェルなので .zshrc を自分で読み、
            // PATH と LANG を自前で構築する。したがって launch_env の注入は不要（§23）
            let _ = launch_env;
            // shell セッションの再開は「同 cwd で新規プロセス起動」（設計書 §11）。
            // ResumeMode は受け取るが引数には影響しない
            let _ = resume;
            Ok(LaunchCommand {
                program: PathBuf::from(program),
                args: vec!["-l".to_string()],
                cwd: cwd.to_path_buf(),
                env,
            })
        }
        CliKind::Claude | CliKind::Codex | CliKind::Custom => Err(AppError::InvalidState(format!(
            "cli_kind {:?} requires M1-4 (worktree + CLI launch)",
            session.cli_kind
        ))),
    }
}

/// PTY の cwd。worktree モードなら worktree_path、そうでなければリポジトリ直上
pub fn resolve_cwd(session: &Session, repo_path: &str) -> PathBuf {
    match session.worktree_path.as_deref() {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(repo_path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CliKind, KanbanStatus, RuntimeState, Session, SessionMode};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    /// `SHELL` 環境変数を読み書きするテスト同士を直列化するためのロック。
    /// 環境変数はプロセス全体で共有されるため、既定の並列実行では他テストと干渉しうる
    /// （フィックス対象レビュー指摘: `cli_args.rs` fix round 1）。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// `SHELL` 環境変数を一時的に上書き/削除し、Drop で必ず元の値へ戻すガード。
    /// panic してもテスト終了時に環境変数が復元される。
    struct ShellEnvGuard {
        original: Option<String>,
    }

    impl ShellEnvGuard {
        fn set(value: &str) -> Self {
            let original = std::env::var("SHELL").ok();
            // SAFETY: 呼び出し元は ENV_LOCK を保持した状態でのみこのガードを作る前提
            // （呼び出し規約はこの struct のドキュメントコメントで明示）。
            // 他スレッドとの同時書き換えはロックで排除している
            unsafe { std::env::set_var("SHELL", value) };
            Self { original }
        }

        fn unset() -> Self {
            let original = std::env::var("SHELL").ok();
            // SAFETY: 上記 set() と同様、ENV_LOCK 保持下でのみ呼ばれる
            unsafe { std::env::remove_var("SHELL") };
            Self { original }
        }
    }

    impl Drop for ShellEnvGuard {
        fn drop(&mut self) {
            match &self.original {
                // SAFETY: ENV_LOCK 保持下でのみ呼ばれる
                Some(value) => unsafe { std::env::set_var("SHELL", value) },
                // SAFETY: 同上
                None => unsafe { std::env::remove_var("SHELL") },
            }
        }
    }

    fn launch_env() -> LaunchEnv {
        LaunchEnv {
            path: "/opt/homebrew/bin:/usr/bin:/bin".to_string(),
            lang: "ja_JP.UTF-8".to_string(),
        }
    }

    fn session(cli_kind: CliKind, cli_command: Option<&str>) -> Session {
        Session {
            id: "00000000-0000-4000-8000-000000000001".to_string(),
            project_id: "00000000-0000-4000-8000-0000000000ff".to_string(),
            title: "fix login".to_string(),
            description: String::new(),
            kanban_status: KanbanStatus::Backlog,
            sort_order: 1.0,
            mode: SessionMode::InPlace,
            branch: None,
            worktree_path: None,
            cli_kind,
            cli_command: cli_command.map(|c| c.to_string()),
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
    fn shell_launches_the_given_program_as_a_login_shell() {
        let cmd = build_launch_command(
            &session(CliKind::Shell, None),
            "/bin/zsh",
            Path::new("/repo"),
            &launch_env(),
            ResumeMode::None,
        )
        .expect("shell");
        assert_eq!(cmd.program, PathBuf::from("/bin/zsh"));
        assert_eq!(cmd.args, vec!["-l".to_string()]);
    }

    #[test]
    fn cwd_comes_from_the_argument_not_from_the_session() {
        // mode == in_place では repo_path が必要だが Session から辿れないので、
        // 呼び出し側が resolve_cwd で決めて引数で渡す（契約 §23）
        let mut s = session(CliKind::Shell, None);
        s.worktree_path = Some("/ignored".to_string());
        let cmd = build_launch_command(
            &s,
            "/bin/zsh",
            Path::new("/repo/.worktrees/session-fix-login"),
            &launch_env(),
            ResumeMode::None,
        )
        .expect("shell");
        assert_eq!(cmd.cwd, PathBuf::from("/repo/.worktrees/session-fix-login"));
    }

    #[test]
    fn shell_injects_kamux_session_id_for_the_hooks_relay() {
        let s = session(CliKind::Shell, None);
        let cmd = build_launch_command(
            &s,
            "/bin/zsh",
            Path::new("/repo"),
            &launch_env(),
            ResumeMode::None,
        )
        .expect("shell");
        assert_eq!(
            cmd.env,
            vec![("KAMUX_SESSION_ID".to_string(), s.id.clone())]
        );
    }

    #[test]
    fn shell_does_not_inject_path_or_lang_because_the_login_shell_builds_them() {
        // 契約 §23: cli_kind == Shell では PATH / LANG の注入が不要。
        // $SHELL -l はインタラクティブシェルとして .zshrc を読み、自分で構築する。
        // claude / codex / nvim（M1-4 / M3-1）はシェルを介さず直接 exec するので注入が必須
        let cmd = build_launch_command(
            &session(CliKind::Shell, None),
            "/bin/zsh",
            Path::new("/repo"),
            &launch_env(),
            ResumeMode::None,
        )
        .expect("shell");
        assert!(
            !cmd.env
                .iter()
                .any(|(key, _)| key == "PATH" || key == "LANG"),
            "actual env: {:?}",
            cmd.env
        );
    }

    #[test]
    fn claude_codex_and_custom_are_not_supported_until_m1_4() {
        for kind in [CliKind::Claude, CliKind::Codex, CliKind::Custom] {
            let err = build_launch_command(
                &session(kind, Some("htop")),
                "/bin/zsh",
                Path::new("/repo"),
                &launch_env(),
                ResumeMode::None,
            )
            .expect_err("out of scope for M1-3");
            assert!(matches!(err, AppError::InvalidState(_)), "actual: {err:?}");
        }
    }

    #[test]
    fn resume_mode_is_accepted_but_ignored_for_shell() {
        // shell セッションの再開は「同 cwd で新規プロセス起動」（設計書 §11）
        for resume in [
            ResumeMode::None,
            ResumeMode::Continue,
            ResumeMode::SessionId("abc-123"),
        ] {
            let cmd = build_launch_command(
                &session(CliKind::Shell, None),
                "/bin/zsh",
                Path::new("/repo"),
                &launch_env(),
                resume,
            )
            .expect("shell");
            assert_eq!(cmd.args, vec!["-l".to_string()]);
        }
    }

    #[test]
    fn resolve_cwd_prefers_worktree_path_over_repo_path() {
        let mut s = session(CliKind::Shell, None);
        assert_eq!(resolve_cwd(&s, "/repo"), PathBuf::from("/repo"));
        s.worktree_path = Some("/repo/.worktrees/session-fix-login".to_string());
        assert_eq!(
            resolve_cwd(&s, "/repo"),
            PathBuf::from("/repo/.worktrees/session-fix-login")
        );
    }

    #[test]
    fn login_shell_is_an_absolute_path() {
        // SHELL を書き換えるテスト（remove_var/set_var）と同時に走ると、$SHELL を直接読む
        // login_shell() がその書き換えと競合しうる（Rust 1.82 以降 env::set_var/remove_var は
        // プロセス全体に影響する unsafe fn）ため、他の SHELL 読み書きテストと同じロックで
        // 直列化する
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(login_shell().starts_with('/'), "actual: {}", login_shell());
    }

    // login_shell() が login_shell_from(std::env::var("SHELL").ok()) に確かに委譲していることを
    // 固定する。login_shell_from の 3 分岐テストは wrapper を経由しないため、wrapper 自体が
    // $SHELL を握りつぶして固定値を返す変異（login_shell_from(Some("/bin/zsh")) など）を検出
    // できない。フォールバック値 `/bin/zsh` とも実行環境の実際の $SHELL とも明確に異なる値
    // （`/tmp/kamux-test-login-shell`）を一時的に設定して読み返すことで弁別力を持たせる
    // （フィックス対象レビュー指摘: 既定の $SHELL=/bin/zsh 環境では旧テストは vacuous だった）。
    #[test]
    fn login_shell_reflects_the_shell_environment_variable() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = ShellEnvGuard::set("/tmp/kamux-test-login-shell");
        assert_eq!(login_shell(), "/tmp/kamux-test-login-shell");
    }

    // login_shell() が $SHELL 未設定時に login_shell_from の既定値 `/bin/zsh` へ確かに
    // フォールバックすることを、wrapper 経由で固定する。
    #[test]
    fn login_shell_falls_back_to_bin_zsh_when_shell_is_unset() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = ShellEnvGuard::unset();
        assert_eq!(login_shell(), "/bin/zsh");
    }

    // login_shell() 自体は $SHELL 環境変数に依存し、書き換えると並列テストと干渉する
    // （レーンの制約）。3 分岐は login_shell_from に切り出した純関数として固定する。
    #[test]
    fn login_shell_from_returns_the_absolute_shell_path_as_is() {
        assert_eq!(
            login_shell_from(Some("/opt/homebrew/bin/fish".to_string())),
            "/opt/homebrew/bin/fish"
        );
    }

    #[test]
    fn login_shell_from_falls_back_when_the_value_is_not_an_absolute_path() {
        // $SHELL が相対パスや空文字のような壊れた値のときはガード節で弾き既定にフォールバックする
        assert_eq!(login_shell_from(Some("zsh".to_string())), "/bin/zsh");
    }

    #[test]
    fn login_shell_from_falls_back_when_shell_is_unset() {
        assert_eq!(login_shell_from(None), "/bin/zsh");
    }
}
