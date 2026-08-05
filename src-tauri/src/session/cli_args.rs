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

/// `cli_kind` が必要とする実行ファイル名。**純粋関数**。
/// Shell / Custom はログインシェル自身を起動するので None。
pub fn binary_name(cli_kind: CliKind) -> Option<&'static str> {
    match cli_kind {
        CliKind::Claude => Some("claude"),
        CliKind::Codex => Some("codex"),
        CliKind::Shell | CliKind::Custom => None,
    }
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
/// `shell` / `claude` / `codex` / `custom` の全 `cli_kind` を実装する（契約 §23）。
pub fn build_launch_command(
    session: &Session,
    program: &str,
    cwd: &Path,
    launch_env: &LaunchEnv,
    resume: ResumeMode<'_>,
) -> AppResult<LaunchCommand> {
    // M2-2 の kamux-relay が自セッションを識別するための環境変数。全 cli_kind 共通（§23）
    let mut env = vec![("KAMUX_SESSION_ID".to_string(), session.id.clone())];

    let args = match session.cli_kind {
        // PTY に繋がった $SHELL -l はインタラクティブシェルなので .zshrc を自分で読み、
        // PATH と LANG を自前で構築する（下の push はスキップされる）。
        // 再開は「同 cwd で新規プロセス起動」（設計書 §11）なので resume は影響しない
        CliKind::Shell => vec!["-l".to_string()],

        // claude / codex は同じ resume フラグ体系を持つ（契約 §12.6）
        CliKind::Claude | CliKind::Codex => match resume {
            ResumeMode::None => Vec::new(),
            ResumeMode::Continue => vec!["--continue".to_string()],
            ResumeMode::SessionId(id) => vec!["--resume".to_string(), id.to_string()],
        },

        // custom: シェル構文（パイプ・クォート・変数展開）をユーザーが書けるよう、
        // 自前でトークン分割せずシェルに委ねる（判断 11）。
        // 再開は「同 cwd で新規プロセス起動」（設計書 §11）なので resume は無視する。
        CliKind::Custom => {
            let command = session.cli_command.as_deref().ok_or_else(|| {
                AppError::InvalidState(format!(
                    "session {} has cli_kind=custom but no cli_command",
                    session.id
                ))
            })?;
            vec!["-l".to_string(), "-c".to_string(), command.to_string()]
        }
    };

    // env の組み立て（契約 §23「env の組み立て責任」）。呼び出し側は一切 push しない。
    //
    // PATH / LANG は cli_kind != Shell のときのみ入れる。Shell は PTY 上の $SHELL -l が
    // インタラクティブシェルとして .zshrc を自分で読み、PATH とロケールを自前で構築する。
    // claude / codex / custom はシェルを介さず、あるいは非インタラクティブシェルで起動する
    // ため .zshrc を読まず、両方の注入が必須。LANG が空のままだと nvim / claude が
    // 日本語ファイル名を化けさせる（契約 §18）。
    //
    // TERM / COLORTERM はここに入れない。所有は `PtySurface::spawn`（契約 §60.6.1 / §60.6.2）。
    if session.cli_kind != CliKind::Shell {
        env.push(("PATH".to_string(), launch_env.path.clone()));
        env.push(("LANG".to_string(), launch_env.lang.clone()));
    }

    Ok(LaunchCommand {
        program: PathBuf::from(program),
        args,
        cwd: cwd.to_path_buf(),
        env,
    })
}

/// PTY の cwd。worktree モードなら worktree_path、そうでなければリポジトリ直上
pub fn resolve_cwd(session: &Session, repo_path: &str) -> PathBuf {
    match session.worktree_path.as_deref() {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(repo_path),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::{CliKind, KanbanStatus, RuntimeState, Session, SessionMode};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    /// `SHELL` 環境変数を読み書きするテスト同士を直列化するためのロック。
    /// 環境変数はプロセス全体で共有されるため、既定の並列実行では他テストと干渉しうる
    /// （フィックス対象レビュー指摘: `cli_args.rs` fix round 1）。
    /// `pub(crate)`: `session::mod` の `plan_agent_spawn` テストも `login_shell()`
    /// （＝ `$SHELL` 読み取り）を踏むため、同じロックで直列化する必要がある
    /// （Task 8 fix round 1 の残り）。新しいロックを作らないこと。
    pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// `SHELL` 環境変数を一時的に上書き/削除し、Drop で必ず元の値へ戻すガード。
    /// panic してもテスト終了時に環境変数が復元される。
    pub(crate) struct ShellEnvGuard {
        original: Option<String>,
    }

    impl ShellEnvGuard {
        pub(crate) fn set(value: &str) -> Self {
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

    // ---- Task 7: binary_name / claude・codex・custom の腕（契約 §23） ----

    fn test_env() -> LaunchEnv {
        LaunchEnv {
            path: "/fake/bin".to_string(),
            lang: "ja_JP.UTF-8".to_string(),
        }
    }

    fn sample_session(cli_kind: CliKind, mode: SessionMode, cli_command: Option<&str>) -> Session {
        Session {
            id: "sess-1".to_string(),
            project_id: "proj-1".to_string(),
            title: "Fix login bug".to_string(),
            description: String::new(),
            kanban_status: KanbanStatus::Backlog,
            sort_order: 1.0,
            mode,
            branch: None,
            worktree_path: None,
            cli_kind,
            cli_command: cli_command.map(|s| s.to_string()),
            claude_session_id: None,
            last_runtime_state: RuntimeState::Idle,
            last_runtime_error: None,
            first_started_at: None,
            archived_at: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    // ---- binary_name（純粋） ----

    #[test]
    fn binary_name_maps_cli_kinds() {
        assert_eq!(binary_name(CliKind::Claude), Some("claude"));
        assert_eq!(binary_name(CliKind::Codex), Some("codex"));
        assert_eq!(
            binary_name(CliKind::Shell),
            None,
            "shell はシェル自身を起動する"
        );
        assert_eq!(
            binary_name(CliKind::Custom),
            None,
            "custom はシェル経由で起動する"
        );
    }

    // ---- build_launch_command（純粋。FS に触らないので偽のパスでよい） ----

    #[test]
    fn claude_has_no_args_in_m1_4() {
        let c = build_launch_command(
            &sample_session(CliKind::Claude, SessionMode::Worktree, None),
            "/bin/claude",
            Path::new("/work"),
            &test_env(),
            ResumeMode::None,
        )
        .expect("cmd");

        assert_eq!(c.program, PathBuf::from("/bin/claude"));
        assert!(c.args.is_empty(), "M1-4 では素の起動（--settings は M2-2）");
        assert_eq!(c.cwd, PathBuf::from("/work"));
    }

    #[test]
    fn claude_supports_resume_by_session_id() {
        // M2-4 が再利用する分岐。純粋関数なので M1-4 で検証できる
        let c = build_launch_command(
            &sample_session(CliKind::Claude, SessionMode::Worktree, None),
            "/bin/claude",
            Path::new("/work"),
            &test_env(),
            ResumeMode::SessionId("abc-123"),
        )
        .expect("cmd");
        assert_eq!(c.args, vec!["--resume".to_string(), "abc-123".to_string()]);
    }

    #[test]
    fn claude_supports_continue_fallback() {
        let c = build_launch_command(
            &sample_session(CliKind::Claude, SessionMode::Worktree, None),
            "/bin/claude",
            Path::new("/work"),
            &test_env(),
            ResumeMode::Continue,
        )
        .expect("cmd");
        assert_eq!(c.args, vec!["--continue".to_string()]);
    }

    #[test]
    fn codex_uses_the_same_resume_flags() {
        let c = build_launch_command(
            &sample_session(CliKind::Codex, SessionMode::Worktree, None),
            "/bin/codex",
            Path::new("/work"),
            &test_env(),
            ResumeMode::SessionId("z9"),
        )
        .expect("cmd");
        assert_eq!(c.program, PathBuf::from("/bin/codex"));
        assert_eq!(c.args, vec!["--resume".to_string(), "z9".to_string()]);
    }

    #[test]
    fn custom_delegates_to_shell_c_without_tokenizing() {
        let c = build_launch_command(
            &sample_session(
                CliKind::Custom,
                SessionMode::Worktree,
                Some("foo --bar | tee 'my log.txt'"),
            ),
            "/bin/zsh",
            Path::new("/work"),
            &test_env(),
            ResumeMode::None,
        )
        .expect("cmd");

        assert_eq!(c.program, PathBuf::from("/bin/zsh"));
        assert_eq!(
            c.args,
            vec![
                "-l".to_string(),
                "-c".to_string(),
                "foo --bar | tee 'my log.txt'".to_string()
            ],
            "パイプやクォートを自前でトークン分割してはならない"
        );
    }

    #[test]
    fn custom_ignores_resume_mode() {
        // shell / custom の再開は「同 cwd で新規プロセス起動」（設計書 §11）
        let c = build_launch_command(
            &sample_session(CliKind::Custom, SessionMode::Worktree, Some("foo")),
            "/bin/zsh",
            Path::new("/work"),
            &test_env(),
            ResumeMode::Continue,
        )
        .expect("cmd");
        assert_eq!(
            c.args,
            vec!["-l".to_string(), "-c".to_string(), "foo".to_string()]
        );
    }

    #[test]
    fn custom_without_command_is_invalid_state() {
        let err = build_launch_command(
            &sample_session(CliKind::Custom, SessionMode::Worktree, None),
            "/bin/zsh",
            Path::new("/work"),
            &test_env(),
            ResumeMode::None,
        )
        .unwrap_err();
        assert!(matches!(err, AppError::InvalidState(_)), "got {err:?}");
    }

    #[test]
    fn in_place_mode_uses_the_cwd_it_is_given() {
        // 契約 §23 のテスト契約: cli_kind × mode × ResumeMode を網羅する
        let c = build_launch_command(
            &sample_session(CliKind::Claude, SessionMode::InPlace, None),
            "/bin/claude",
            Path::new("/repo"),
            &test_env(),
            ResumeMode::None,
        )
        .expect("cmd");
        assert_eq!(c.cwd, PathBuf::from("/repo"));
    }

    #[test]
    fn m1_4_arms_get_path_and_lang_injected_and_keep_session_id() {
        // claude / codex / custom はシェルの .zshrc を経由しないので PATH / LANG 注入が必須。
        // 同時に、M1-3 が入れた KAMUX_SESSION_ID を潰していないことを固定する
        // （env を作り直すと M2-2 の kamux-relay が壊れる）。
        // なお shell の腕が PATH / LANG を注入しないことのテストは M1-3 が所有する
        // （shell_does_not_inject_path_or_lang_because_the_login_shell_builds_them）。
        for (kind, cmd) in [
            (CliKind::Claude, None),
            (CliKind::Codex, None),
            (CliKind::Custom, Some("foo")),
        ] {
            let c = build_launch_command(
                &sample_session(kind, SessionMode::Worktree, cmd),
                "/bin/prog",
                Path::new("/work"),
                &test_env(),
                ResumeMode::None,
            )
            .expect("cmd");

            assert!(
                c.env.iter().any(|(n, v)| n == "PATH" && v == "/fake/bin"),
                "{kind:?} must get PATH injected, got {:?}",
                c.env
            );
            // LANG も必須。空のままだと日本語ファイル名が化ける（契約 §18）。
            // M1-3 の shell 側テストが「PATH も LANG も入らない」を固定しているので、
            // こちらは「両方入る」を固定して非対称性を両側から挟む。
            assert!(
                c.env.iter().any(|(n, v)| n == "LANG" && v == "ja_JP.UTF-8"),
                "{kind:?} must get LANG injected, got {:?}",
                c.env
            );
            assert!(
                c.env
                    .iter()
                    .any(|(n, v)| n == "KAMUX_SESSION_ID" && v == "sess-1"),
                "{kind:?} must keep KAMUX_SESSION_ID, got {:?}",
                c.env
            );
        }
    }

    #[test]
    fn env_carries_session_id_path_and_lang() {
        let c = build_launch_command(
            &sample_session(CliKind::Claude, SessionMode::Worktree, None),
            "/bin/claude",
            Path::new("/work"),
            &test_env(),
            ResumeMode::None,
        )
        .expect("cmd");

        let get = |k: &str| c.env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("KAMUX_SESSION_ID"), Some("sess-1".to_string()));
        assert_eq!(get("PATH"), Some("/fake/bin".to_string()));
        assert_eq!(
            get("LANG"),
            Some("ja_JP.UTF-8".to_string()),
            "空 LANG は日本語ファイル名を化けさせる"
        );
    }
}
