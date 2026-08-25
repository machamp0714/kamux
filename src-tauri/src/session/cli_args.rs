use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::hooks_srv::HooksRuntime;
use crate::model::{CliKind, Session, SessionMode};
use crate::pty::launch_env::LaunchEnv;

/// 再開時にどうやって会話を復元するかの決定(第1部 §3 分岐表)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ResumePlan {
    ClaudeResume { claude_session_id: String },
    ClaudeContinue,
    FreshStart { reason: FreshStartReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FreshStartReason {
    /// claude / in_place / claude_session_id 欠損。
    /// 同一 cwd に複数セッションが同居しうるため --continue を使わない。
    AmbiguousInPlaceConversation,
    /// cli_kind が会話復元に対応しない(shell / custom / codex)。
    NoConversationRestore,
}

/// 第1部 §3 の分岐表をそのまま実装した純粋関数。
/// ファイルシステム・環境変数・時刻に一切触れない。
pub fn resume_plan(session: &Session) -> ResumePlan {
    match session.cli_kind {
        CliKind::Claude => match (&session.claude_session_id, session.mode) {
            (Some(id), _) => ResumePlan::ClaudeResume {
                claude_session_id: id.clone(),
            },
            (None, SessionMode::Worktree) => ResumePlan::ClaudeContinue,
            (None, SessionMode::InPlace) => ResumePlan::FreshStart {
                reason: FreshStartReason::AmbiguousInPlaceConversation,
            },
        },
        // codex は resume フラグが未確認のため、保守的に会話復元を試みない。
        CliKind::Codex | CliKind::Shell | CliKind::Custom => ResumePlan::FreshStart {
            reason: FreshStartReason::NoConversationRestore,
        },
    }
}

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

/// `resume_plan()` の決定を `build_launch_command` が受け取る形へ移す**純粋関数**。
///
/// 戻り値の `ResumeMode<'_>` は引数の `ResumePlan` が持つ `String` を借りるので、
/// 呼び出し側は `ResumePlan` を先に `let` で束縛してからこの関数を呼ぶ
/// （一時値のままだと文の終わりで drop される）。
///
/// **`cli_kind` をここで見ない。** 分岐表(第1部 §3)の判断は `resume_plan()` が
/// 1 箇所で持っており、この関数はその決定を写すだけである。`FreshStart` は
/// 理由によらず `ResumeMode::None` へ落ちる —— codex / shell / custom は
/// `resume_plan()` が必ず `FreshStart` を返す(行 5〜16)ので、
/// `build_launch_command` の `CliKind::Claude | CliKind::Codex` の腕へ
/// 非 `None` の `ResumeMode` が届く経路ができない。
pub fn resume_mode(plan: &ResumePlan) -> ResumeMode<'_> {
    match plan {
        ResumePlan::ClaudeResume { claude_session_id } => ResumeMode::SessionId(claude_session_id),
        ResumePlan::ClaudeContinue => ResumeMode::Continue,
        ResumePlan::FreshStart { .. } => ResumeMode::None,
    }
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

/// kamux が購読する hook イベント（契約 §12.4）。
///
/// `PermissionRequest` は「ユーザーの承認待ち」の最も直接的な信号なので
/// `Notification` と併せて必ず登録する。
/// `PermissionDenied` は対応する runtime_state が契約 §2 に無いため登録しない
/// （§83.6.1 / §84.5）。
pub const HOOK_EVENTS: [&str; 4] = ["SessionStart", "Notification", "PermissionRequest", "Stop"];

/// hook 1 個あたりの制限時間（秒）。既定の 600 秒でぶら下がるのを避ける。
pub const HOOK_TIMEOUT_SECS: u64 = 5;

/// hook の command は sh -c 経由で実行される。パスに空白が含まれても壊れないよう包む。
pub fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `--settings` に渡す JSON を組み立てる（設計 §4）。
///
/// 契約 §12.2: この設定はユーザーの settings.json を**置換しない**。マージされる。
/// ユーザー自身の Stop / Notification hook も同時に発火する前提で、
/// kamux 側の hook は副作用のない転送だけを行う。
pub fn build_hook_settings(relay_bin: &Path) -> serde_json::Value {
    let quoted = shell_single_quote(&relay_bin.to_string_lossy());

    let mut hooks = serde_json::Map::new();
    for event in HOOK_EVENTS {
        hooks.insert(
            event.to_string(),
            serde_json::json!([{
                "hooks": [{
                    "type": "command",
                    // hook 種別を argv 第 1 引数で渡す（契約 §84.1）。
                    "command": format!("{quoted} {event}"),
                    "timeout": HOOK_TIMEOUT_SECS,
                }]
            }]),
        );
    }

    serde_json::json!({ "hooks": serde_json::Value::Object(hooks) })
}

/// settings JSON をファイルへ書く。アプリ起動につき 1 回。
pub fn write_hook_settings_file(path: &Path, relay_bin: &Path) -> AppResult<()> {
    let text = serde_json::to_string_pretty(&build_hook_settings(relay_bin))
        .map_err(|e| AppError::Io(format!("failed to serialize hook settings: {e}")))?;
    std::fs::write(path, text).map_err(|e| {
        AppError::Io(format!(
            "failed to write hook settings {}: {e}",
            path.display()
        ))
    })
}

/// 契約 §12.1: リレーは KAMUX_SESSION_ID で自セッションを識別する。
pub const ENV_SESSION_ID: &str = "KAMUX_SESSION_ID";
/// 契約への追加提案 B: relay はアプリの pid を知らないのでソケットパスを env で受け取る。
pub const ENV_HOOKS_SOCK: &str = "KAMUX_HOOKS_SOCK";
/// 契約 §30.1 / §30.2: shim ディレクトリの絶対パス。**shim はこの env が
/// PATH に居るかどうかではなく、`KAMUX_HOOKS_SETTINGS` の有無で `--settings` を
/// 足すか決める。** shim 有効時のみ入る。
pub const ENV_SHIM_DIR: &str = "KAMUX_SHIM_DIR";
/// 契約 §30.1 / §30.2: shim が `--settings` に渡す settings ファイルの絶対パス。
/// **shim が有効なのは kamux が起こした PTY の中だけ、という性質をこの env の有無で
/// 構造的に保証する。**
pub const ENV_HOOKS_SETTINGS: &str = "KAMUX_HOOKS_SETTINGS";

/// 契約 §64.5.1 の「`PATH` の最終値」の表を 1 箇所で持つ純関数。
///
/// `process_path` は**現プロセスの `PATH`**（契約 §30.2 の逐語）。`cli_kind == Shell`
/// のときだけ使う —— その腕は §23 により `launch_env.path` を持たないので、土台に
/// できるのは現プロセスの `PATH` だけである。
///
/// **`env` へ 2 つ目の `PATH` を push しない。** `Claude` / `Codex` は
/// `build_launch_command` が既に入れた対を**書き換える** —— `Vec<(String, String)>`
/// に `PATH` が 2 つ並ぶと、どちらが勝つかが並び順に依存する（§64.5.1 が
/// `hook_env_vars` について却下したのと同じ形の事故）。
fn prepend_shim_dir_to_path(
    cli_kind: CliKind,
    shim_dir: &str,
    process_path: &str,
    env: &mut Vec<(String, String)>,
) {
    match cli_kind {
        // §64.5: custom は `$SHELL -l -c "<cli_command>"` で起動し、`-l` が
        // path_helper を通すので shim は末尾へ落ちる。手前に本物の claude が
        // 居るかどうかで発火したりしなかったりする「動くこともある状態」を
        // 作らないため、入れない。
        CliKind::Custom => {}
        // §23 が `PATH` を入れない唯一の腕。ここでだけ対を新しく push する。
        CliKind::Shell => env.push(("PATH".to_string(), format!("{shim_dir}:{process_path}"))),
        CliKind::Claude | CliKind::Codex => {
            for (key, value) in env.iter_mut() {
                if key == "PATH" {
                    *value = format!("{shim_dir}:{value}");
                }
            }
        }
    }
}

/// claude の argv に足す hooks 引数。
///
/// `--settings` はここでは `Command::arg` 経由(cmd.arg)で子プロセスへ渡り、
/// シェルを経由しないので、パスに空白や `'` が含まれてもクォートは不要かつ
/// 有害である(§60.6.1 の `PtySurface::spawn` が `CommandBuilder::arg` で
/// argv 要素を 1 つずつ積む。`spec.args` の各要素は execve の argv[i] になる)。
pub fn claude_hook_args(hooks: Option<&HooksRuntime>) -> Vec<String> {
    match hooks {
        Some(h) => vec![
            "--settings".to_string(),
            h.settings_path.to_string_lossy().into_owned(),
        ],
        None => Vec::new(),
    }
}

/// PTY spawn 時に注入する環境変数。
/// 契約 §12.3 のとおり hook プロセスへ完全継承される。
///
/// **`KAMUX_SESSION_ID` は返さない**（契約 §102.3）。所有者は `build_launch_command`
/// であり、この関数を第 1 引数の `kamux_session_id` ごと落とすことで、対を返す実装を
/// 構造的に書けなくしてある。引数を残して散文で禁じる形は採らない
/// （§102.3 の却下記録）。
pub fn hook_env_vars(hooks: Option<&HooksRuntime>) -> Vec<(String, String)> {
    let Some(h) = hooks else {
        return Vec::new();
    };
    let mut env = vec![
        (
            ENV_HOOKS_SOCK.to_string(),
            h.socket_path.to_string_lossy().into_owned(),
        ),
        (
            ENV_HOOKS_SETTINGS.to_string(),
            h.settings_path.to_string_lossy().into_owned(),
        ),
    ];
    // shim 無効時は対そのものを入れない。空文字を入れると、shim スクリプトの
    // 「KAMUX_SHIM_DIR を除いた PATH」の比較が空文字と突き合わさって壊れる。
    if let Some(shim_dir) = h.shim_dir.as_ref() {
        env.push((
            ENV_SHIM_DIR.to_string(),
            shim_dir.to_string_lossy().into_owned(),
        ));
    }
    env
}

/// `build_launch_command` が組み立てた結果に hooks 由来の値を重ねる。**argv と env で
/// 射程が違う**（契約 §30.2）。
///
/// - **argv（`--settings`）は `cli_kind == Claude` 限定。** claude 専用フラグであり、
///   §30.2 の env 表は argv を射程にしていない
/// - **env（`KAMUX_HOOKS_SOCK`）は全 `cli_kind` 共通**（§30.2 / §31.2 の既往ドリフト訂正）。
///   shell のスクラッチ端末から手で起動した claude の hook も relay に届く必要があるため、
///   `cli_kind` で絞ってはならない
///
/// `build_launch_command` 自身のシグネチャは変えない（契約 §23 / §30.2.1）——
/// 5 引数のままでは `HooksRuntime` に辿り着けないため、この関数を後段の合流点として
/// 1 箇所に閉じる（§31.4 の「呼び出し側は値を組み立てず、`hook_env_vars` の結果を
/// 合流させるだけ」）。`KAMUX_SESSION_ID` はここでは足さない —— 所有は
/// `build_launch_command` である（§102.3）。
pub fn apply_hooks(
    session: &Session,
    mut cmd: LaunchCommand,
    hooks: Option<&HooksRuntime>,
) -> LaunchCommand {
    if session.cli_kind == CliKind::Claude {
        cmd.args.extend(claude_hook_args(hooks));
    }
    cmd.env.extend(hook_env_vars(hooks));

    // `PATH` への shim ディレクトリの prepend（契約 §64.5 / §64.5.1）。
    //
    // **ここに置く理由**: `PATH` の最終値を組むには `cli_kind` と `HooksRuntime` と
    // 組み立て済みの `env` の 3 つが同時に見える場所が要る。`build_launch_command` は
    // §23 の 5 引数固定で `HooksRuntime` に辿り着けず、純粋関数なので現プロセスの
    // `PATH` も読めない。`hook_env_vars` は §64.5.1 が `PATH` の対を返すことを名指しで
    // 却下している（`cli_kind` も見られない）。**シグネチャ固定の下で残るのは、
    // この doc が既に「後段の合流点」と呼んでいるこの関数だけである。**
    // 契約 §64.5.1 の逐語は「`PATH` の最終値は `build_launch_command` が組む」だが、
    // その文は同節が想定した「`hooks` を見られる `build_launch_command`」を前提に
    // している —— 5 引数を変えない限り成立しない。**主旨（受け渡し口を 1 本に絞る /
    // `PATH` の対を 2 つ作らない）はこの置き場所で満たされる。**
    if let Some(shim_dir) = hooks.and_then(|h| h.shim_dir.as_ref()) {
        let process_path = std::env::var("PATH").unwrap_or_default();
        prepend_shim_dir_to_path(
            session.cli_kind,
            &shim_dir.to_string_lossy(),
            &process_path,
            &mut cmd.env,
        );
    }
    cmd
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
            heuristics_enabled: true,
            silence_timeout_secs: 30,
            is_scratch: false,
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
            heuristics_enabled: true,
            silence_timeout_secs: 30,
            is_scratch: false,
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

    // ---- Task 9: --settings に渡す hooks 設定 JSON の生成（契約 §12.2 / §12.4） ----

    #[test]
    fn quotes_plain_paths() {
        assert_eq!(
            shell_single_quote("/usr/local/bin/kamux-relay"),
            "'/usr/local/bin/kamux-relay'"
        );
    }

    #[test]
    fn quotes_paths_with_spaces() {
        assert_eq!(
            shell_single_quote("/Applications/My App.app/Contents/MacOS/kamux-relay"),
            "'/Applications/My App.app/Contents/MacOS/kamux-relay'"
        );
    }

    #[test]
    fn escapes_embedded_single_quotes() {
        assert_eq!(
            shell_single_quote("/tmp/it's/relay"),
            r#"'/tmp/it'\''s/relay'"#
        );
    }

    #[test]
    fn builds_settings_for_the_four_hook_events() {
        let v = build_hook_settings(Path::new("/opt/kamux/kamux-relay"));

        let hooks = v["hooks"].as_object().expect("hooks object");
        assert_eq!(
            hooks.len(),
            4,
            "SessionStart / Notification / PermissionRequest / Stop"
        );
        // 契約 §12.4: PermissionRequest は waiting_input の最も直接的な信号なので必須。
        // このキー名が実際に有効かは未確認（表 #13b）。Task 17 Step 4 が発火で検証する。
        assert!(hooks.contains_key("PermissionRequest"));
        // PermissionDenied は登録しない（対応する runtime_state が無い。契約 §83.6.1 / §84.5）
        assert!(!hooks.contains_key("PermissionDenied"));

        // イベント名は HOOK_EVENTS から再導出せず、契約 §12.4 / §83.6.1 / §84.5 の
        // 4 値をリテラルで固定する。production の HOOK_EVENTS 配列を変異させても、
        // ここが道連れで変異すると検出力を失う（契約 §90.2 のフィクスチャ規律）。
        for event in ["SessionStart", "Notification", "PermissionRequest", "Stop"] {
            let entries = hooks[event].as_array().expect("array of matcher groups");
            assert_eq!(entries.len(), 1);
            // matcher は書かない（設計 §4 / 未確認事実 #11）
            assert!(
                entries[0].get("matcher").is_none(),
                "{event} must not carry a matcher"
            );

            let inner = entries[0]["hooks"].as_array().expect("array of hooks");
            assert_eq!(inner.len(), 1);
            assert_eq!(inner[0]["type"], "command");
            assert_eq!(
                inner[0]["command"],
                format!("'/opt/kamux/kamux-relay' {event}")
            );
            assert_eq!(inner[0]["timeout"], 5);
        }
    }

    #[test]
    fn settings_json_is_written_to_disk_and_reparses() {
        let path =
            std::env::temp_dir().join(format!("kamux-settings-test-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);

        write_hook_settings_file(&path, Path::new("/opt/kamux/kamux-relay")).expect("write");

        let text = std::fs::read_to_string(&path).expect("read");
        let v: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(
            v["hooks"]["Stop"][0]["hooks"][0]["command"],
            "'/opt/kamux/kamux-relay' Stop"
        );

        let _ = std::fs::remove_file(&path);
    }

    // ---- Task 10: claude の argv と環境変数への hooks 注入（契約 §12.1 / §61.2） ----

    /// フィクスチャのパスは意図的に空白と `'` を含む
    /// （`claude_hook_args` / `hook_env_vars` の doc コメントが「シェルを経由しないため
    /// 空白や `'` を含んでもクォート不要かつ有害」と主張しており、その主張を検査するため。
    /// フィックス対象レビュー指摘: `cli_args.rs` Task 10 fix round 1）。
    /// hooks も shim も**有効**なランタイム。`shim_dir` は `test_env().path`
    /// （`/fake/bin`）の部分文字列にならない値にしてある —— 部分文字列だと
    /// 「prepend されたか」と「元のままか」を `starts_with` で弁別できなくなる。
    fn fake_runtime() -> HooksRuntime {
        HooksRuntime {
            socket_path: PathBuf::from("/tmp/kamux's dir/kamux-hooks-4321.sock"),
            settings_path: PathBuf::from("/tmp/kamux's dir/kamux-hooks-4321.settings.json"),
            relay_bin: PathBuf::from("/opt/kamux/kamux-relay"),
            shim_dir: Some(PathBuf::from("/tmp/kamux's dir/shim")),
        }
    }

    /// hooks は有効・shim は**無効**（`shim_dir == None`）。契約 §64.5.1 の
    /// 「shim 無効時は §23 / §30.2 の既存規定のまま」を見るテストが使う。
    fn fake_runtime_without_shim() -> HooksRuntime {
        HooksRuntime {
            shim_dir: None,
            ..fake_runtime()
        }
    }

    #[test]
    fn adds_settings_flag_when_hooks_are_enabled() {
        let args = claude_hook_args(Some(&fake_runtime()));
        assert_eq!(
            args,
            vec![
                "--settings".to_string(),
                "/tmp/kamux's dir/kamux-hooks-4321.settings.json".to_string()
            ]
        );
    }

    #[test]
    fn adds_nothing_when_hooks_are_disabled() {
        assert!(claude_hook_args(None).is_empty());
        assert!(hook_env_vars(None).is_empty());
    }

    /// 契約 §64.5.1 の逐語: 「`hook_env_vars` が返し続けるのは `KAMUX_SHIM_DIR` /
    /// `KAMUX_HOOKS_SETTINGS` / `KAMUX_HOOKS_SOCK` である。」
    ///
    /// **`contains` ではなく完全一致で見る。** 緩めると、`PATH` の対を返させる変異
    /// （契約 §64.5.1 が名指しで却下した形）がここを素通りする。
    /// 契約 §102.3: `KAMUX_SESSION_ID` は返さない（所有者は `build_launch_command`）。
    #[test]
    fn injects_sock_settings_and_shim_dir_and_nothing_else() {
        let env = hook_env_vars(Some(&fake_runtime()));
        assert_eq!(
            env,
            vec![
                (
                    "KAMUX_HOOKS_SOCK".to_string(),
                    "/tmp/kamux's dir/kamux-hooks-4321.sock".to_string()
                ),
                (
                    "KAMUX_HOOKS_SETTINGS".to_string(),
                    "/tmp/kamux's dir/kamux-hooks-4321.settings.json".to_string()
                ),
                (
                    "KAMUX_SHIM_DIR".to_string(),
                    "/tmp/kamux's dir/shim".to_string()
                ),
            ]
        );
    }

    /// shim 無効時（`shim_dir == None`）は `KAMUX_SHIM_DIR` を返さない。
    /// **空文字の対を返す**変異（`unwrap_or_default()` 相当）は、shim スクリプトの
    /// PATH 除去を空文字と比較させて壊すので、ここで弁別する。
    #[test]
    fn omits_the_shim_dir_env_when_the_shim_is_disabled() {
        let env = hook_env_vars(Some(&fake_runtime_without_shim()));
        assert_eq!(
            env,
            vec![
                (
                    "KAMUX_HOOKS_SOCK".to_string(),
                    "/tmp/kamux's dir/kamux-hooks-4321.sock".to_string()
                ),
                (
                    "KAMUX_HOOKS_SETTINGS".to_string(),
                    "/tmp/kamux's dir/kamux-hooks-4321.settings.json".to_string()
                ),
            ]
        );
    }

    /// 契約 §64.5.1 の「`PATH` の最終値」の表そのもの（4 行）。`process_path` を
    /// 引数で受ける純関数に閉じてあるので、実行環境の `PATH` に依存しない。
    #[test]
    fn the_path_table_of_the_contract_holds_for_the_four_cli_kinds() {
        let cases = [
            // (cli_kind, build_launch_command が入れた PATH, 期待値)
            (CliKind::Claude, Some("/fake/bin"), Some("/shim:/fake/bin")),
            (CliKind::Codex, Some("/fake/bin"), Some("/shim:/fake/bin")),
            // Shell は §23 により PATH の対を持たない。ここで初めて 1 つだけ push される
            (CliKind::Shell, None, Some("/shim:/proc/path")),
            // §64.5: custom には shim を入れない
            (CliKind::Custom, Some("/fake/bin"), Some("/fake/bin")),
        ];
        for (cli_kind, base, expected) in cases {
            let mut env: Vec<(String, String)> = vec![("LANG".to_string(), "C".to_string())];
            if let Some(path) = base {
                env.push(("PATH".to_string(), path.to_string()));
            }

            prepend_shim_dir_to_path(cli_kind, "/shim", "/proc/path", &mut env);

            let paths: Vec<&str> = env
                .iter()
                .filter(|(k, _)| k == "PATH")
                .map(|(_, v)| v.as_str())
                .collect();
            assert_eq!(
                paths,
                expected.into_iter().collect::<Vec<_>>(),
                "cli_kind={cli_kind:?} の PATH が契約 §64.5.1 の表と違う: {env:?}"
            );
        }
    }

    /// 契約 §64.5.1: shim 無効時は §23 / §30.2 の既存規定のまま。
    /// `Shell` に `PATH` の対が生えないことも同時に見る。
    #[test]
    fn the_path_is_untouched_when_the_shim_is_disabled() {
        for (cli_kind, cmd) in apply_hooks_for_the_four_cli_kinds(&fake_runtime_without_shim()) {
            let paths: Vec<&str> = cmd
                .env
                .iter()
                .filter(|(k, _)| k == "PATH")
                .map(|(_, v)| v.as_str())
                .collect();
            let expected: Vec<&str> = match cli_kind {
                CliKind::Shell => Vec::new(),
                _ => vec!["/fake/bin"],
            };
            assert_eq!(paths, expected, "cli_kind={cli_kind:?}: {:?}", cmd.env);
        }
    }

    /// 契約 §64.8 の (a) / (b) / (c) を 1 本で見る。
    ///
    /// - (a) `Custom` の `PATH` は `launch_env.path` **そのもの**（完全一致）
    /// - (b) `Claude` / `Codex` の `PATH` は `{shim_dir}:{launch_env.path}`
    /// - (c) `env` に `PATH` の対が 2 つ現れない（`filter` の結果が 1 要素）
    ///
    /// `Shell` は §30.2 の逐語のとおり `{shim_dir}:{現プロセスの PATH}` である ——
    /// `launch_env.path`（`/fake/bin`）を土台にする変異をここで弁別する。
    #[test]
    fn the_shim_dir_is_prepended_to_path_for_every_cli_kind_except_custom() {
        let process_path = std::env::var("PATH").unwrap_or_default();
        for (cli_kind, cmd) in apply_hooks_for_the_four_cli_kinds(&fake_runtime()) {
            let paths: Vec<&str> = cmd
                .env
                .iter()
                .filter(|(k, _)| k == "PATH")
                .map(|(_, v)| v.as_str())
                .collect();
            let expected = match cli_kind {
                CliKind::Custom => "/fake/bin".to_string(),
                CliKind::Shell => format!("/tmp/kamux's dir/shim:{process_path}"),
                CliKind::Claude | CliKind::Codex => "/tmp/kamux's dir/shim:/fake/bin".to_string(),
            };
            assert_eq!(
                paths,
                vec![expected.as_str()],
                "cli_kind={cli_kind:?} の PATH: {:?}",
                cmd.env
            );
        }
    }

    /// 契約 §30.2 の env 表: `KAMUX_SHIM_DIR` / `KAMUX_HOOKS_SETTINGS` は
    /// **全 `cli_kind` 共通**である（`PATH` に居なければ不活性なので、例外を
    /// 2 つに増やす理由が無い。§64.5）。`Custom` にも入ることを固定する。
    #[test]
    fn the_shim_env_vars_are_injected_for_claude_codex_shell_and_custom() {
        for (cli_kind, cmd) in apply_hooks_for_the_four_cli_kinds(&fake_runtime()) {
            for (key, value) in [
                ("KAMUX_SHIM_DIR", "/tmp/kamux's dir/shim"),
                (
                    "KAMUX_HOOKS_SETTINGS",
                    "/tmp/kamux's dir/kamux-hooks-4321.settings.json",
                ),
            ] {
                assert!(
                    cmd.env.contains(&(key.to_string(), value.to_string())),
                    "cli_kind={cli_kind:?} の env に {key} が入っていない: {:?}",
                    cmd.env
                );
            }
        }
    }

    // ---- Task 11: M1-4 の claude 起動経路への結線（契約 §31.4 / §102） ----

    /// `apply_hooks` の第 3 引数用。`Some` のときだけ claude の args / env に
    /// hooks 由来の値を重ねる。
    fn claude_session(hooks_enabled: bool) -> (Session, Option<HooksRuntime>) {
        let session = sample_session(CliKind::Claude, SessionMode::InPlace, None);
        let hooks = if hooks_enabled {
            Some(fake_runtime())
        } else {
            None
        };
        (session, hooks)
    }

    #[test]
    fn claude_command_carries_settings_flag_and_hook_env() {
        let (session, hooks) = claude_session(true);
        let base = build_launch_command(
            &session,
            "/opt/homebrew/bin/claude",
            Path::new("/repo"),
            &test_env(),
            ResumeMode::None,
        )
        .expect("build");
        let cmd = apply_hooks(&session, base, hooks.as_ref());

        let pos = cmd
            .args
            .iter()
            .position(|a| a == "--settings")
            .expect("--settings must be present");
        assert_eq!(
            cmd.args[pos + 1],
            "/tmp/kamux's dir/kamux-hooks-4321.settings.json"
        );
        assert!(cmd
            .env
            .contains(&("KAMUX_SESSION_ID".to_string(), session.id.clone())));
        assert!(cmd.env.contains(&(
            "KAMUX_HOOKS_SOCK".to_string(),
            "/tmp/kamux's dir/kamux-hooks-4321.sock".to_string()
        )));
    }

    /// `CliKind` の 4 variant（`Claude` / `Codex` / `Shell` / `Custom`）について
    /// `apply_hooks` を通した結果を、`cli_kind` と対で返す。
    ///
    /// **この関数が担保するのは「`CliKind` に 5 つ目の variant が増えたときに
    /// `cli_command` を決める `match` がコンパイルエラーになる」という接触点だけである。**
    /// 下の配列リテラルへの追加は依然として手作業であり、型はそれを担保しない
    /// （配列から要素を 1 つ落としても、この関数はコンパイルも実行も通る）。
    /// だからこの関数名も呼び出し側のテスト名も「every」「all」を名乗らず、
    /// 4 種を明示的に列挙している。
    ///
    /// 網羅を機構で担保する設計（index/count によるチェック等）はあえて採らない。
    /// `CliKind` は契約 §2 が `Claude` / `Codex` / `Shell` / `Custom` の 4 値で固定して
    /// おり、5 つ目を足すこと自体が契約改訂であって、その改訂は §30.2 の「全 `cli_kind`
    /// 共通」の env 表を必ず通る —— 機構が守ろうとしている経路を、契約改訂そのものが
    /// 通過するので過剰設計になる。
    ///
    /// `program` は `--settings` / env のどちらの assert にも効かないので `/bin/zsh` 固定。
    fn apply_hooks_for_the_four_cli_kinds(hooks: &HooksRuntime) -> Vec<(CliKind, LaunchCommand)> {
        // CliKind に variant を足したら、下の match（コンパイルエラーで気づける）と
        // この配列（気づけない。手作業で追記すること）の両方に足すこと。
        let every = [
            CliKind::Claude,
            CliKind::Codex,
            CliKind::Shell,
            CliKind::Custom,
        ];
        every
            .into_iter()
            .map(|cli_kind| {
                let cli_command = match cli_kind {
                    CliKind::Custom => Some("echo hi"),
                    CliKind::Claude | CliKind::Codex | CliKind::Shell => None,
                };
                let session = sample_session(cli_kind, SessionMode::InPlace, cli_command);
                let base = build_launch_command(
                    &session,
                    "/bin/zsh",
                    Path::new("/repo"),
                    &test_env(),
                    ResumeMode::None,
                )
                .expect("build");
                (cli_kind, apply_hooks(&session, base, Some(hooks)))
            })
            .collect()
    }

    /// 契約 §30.2 の env 表: `KAMUX_HOOKS_SOCK` は**全 `cli_kind` 共通**で入る
    /// （shell のスクラッチ端末から手で起動した claude の hook も届く必要があるため）。
    /// 値まで比較するのは、キーの存在だけだと空文字や別パスを入れる潰しを捕まえられないため。
    #[test]
    fn hooks_sock_env_is_injected_for_claude_codex_shell_and_custom() {
        for (cli_kind, cmd) in apply_hooks_for_the_four_cli_kinds(&fake_runtime()) {
            assert!(
                cmd.env.contains(&(
                    "KAMUX_HOOKS_SOCK".to_string(),
                    "/tmp/kamux's dir/kamux-hooks-4321.sock".to_string()
                )),
                "cli_kind={cli_kind:?} の env に KAMUX_HOOKS_SOCK が入っていない: {:?}",
                cmd.env
            );
        }
    }

    /// 契約 §30.2 の分界のうち **argv 側**: `--settings` は claude 専用フラグであり、
    /// §30.2 の env 表は argv を射程にしていない。`cli_kind == Claude` のときだけ付き、
    /// 他の 3 種には付かないことを固定する（env 側の主張はここではしない —— それは
    /// `hooks_sock_env_is_injected_for_claude_codex_shell_and_custom` が別の規定として持つ）。
    #[test]
    fn settings_flag_is_injected_for_claude_only_among_the_four_cli_kinds() {
        for (cli_kind, cmd) in apply_hooks_for_the_four_cli_kinds(&fake_runtime()) {
            assert_eq!(
                cmd.args.iter().any(|a| a == "--settings"),
                cli_kind == CliKind::Claude,
                "cli_kind={cli_kind:?} の args: {:?}",
                cmd.args
            );
        }
    }

    #[test]
    fn claude_command_works_without_hooks() {
        let (session, _none) = claude_session(false);
        let base = build_launch_command(
            &session,
            "/opt/homebrew/bin/claude",
            Path::new("/repo"),
            &test_env(),
            ResumeMode::None,
        )
        .expect("build");
        let cmd = apply_hooks(&session, base, None);

        assert!(!cmd.args.iter().any(|a| a == "--settings"));
        // 契約 §102: 所有者は build_launch_command であり、hooks == None でも
        // KAMUX_SESSION_ID はちょうど 1 個入る。入らないのは KAMUX_HOOKS_SOCK のほう
        // （§30.2 の「全 cli_kind 共通」/ §12.1）。
        assert_eq!(
            cmd.env
                .iter()
                .filter(|(k, _)| k == "KAMUX_SESSION_ID")
                .count(),
            1
        );
        assert!(!cmd.env.iter().any(|(k, _)| k == "KAMUX_HOOKS_SOCK"));
    }

    /// 契約 §102.7: 合流後の env にキー重複が無いことを、claude + hooks 有効の
    /// 組み合わせ（全書き手が同時発火する条件）で見る。キー名は固定しない
    /// （M3-4 が KAMUX_SHIM_DIR / KAMUX_HOOKS_SETTINGS を足しても同じテストが守るため）。
    #[test]
    fn claude_hooks_do_not_duplicate_any_env_key() {
        let (session, hooks) = claude_session(true);
        let base = build_launch_command(
            &session,
            "/opt/homebrew/bin/claude",
            Path::new("/repo"),
            &test_env(),
            ResumeMode::None,
        )
        .expect("build");
        let cmd = apply_hooks(&session, base, hooks.as_ref());

        let mut seen: Vec<&str> = Vec::new();
        for (key, _) in &cmd.env {
            let key = key.as_str();
            assert!(
                !seen.contains(&key),
                "key {key} appears more than once in {:?}",
                cmd.env
            );
            seen.push(key);
        }
    }
}

#[cfg(test)]
mod resume_plan_tests {
    use super::*;
    use crate::model::{CliKind, KanbanStatus, RuntimeState, Session, SessionMode};

    /// テスト用の Session を組み立てる。分岐に関係するフィールドだけ引数で受ける。
    fn session(cli_kind: CliKind, claude_session_id: Option<&str>, mode: SessionMode) -> Session {
        Session {
            id: "11111111-1111-4111-8111-111111111111".to_string(),
            project_id: "22222222-2222-4222-8222-222222222222".to_string(),
            title: "fix login".to_string(),
            description: String::new(),
            kanban_status: KanbanStatus::InProgress,
            sort_order: 1.0,
            mode,
            branch: match mode {
                SessionMode::Worktree => Some("session/fix-login".to_string()),
                SessionMode::InPlace => None,
            },
            worktree_path: match mode {
                SessionMode::Worktree => Some("/repo/.worktrees/session-fix-login".to_string()),
                SessionMode::InPlace => None,
            },
            cli_kind,
            cli_command: match cli_kind {
                CliKind::Custom => Some("my-agent --flag".to_string()),
                _ => None,
            },
            claude_session_id: claude_session_id.map(|s| s.to_string()),
            last_runtime_state: RuntimeState::Interrupted,
            last_runtime_error: None,
            first_started_at: None,
            heuristics_enabled: true,
            silence_timeout_secs: 30,
            is_scratch: false,
            archived_at: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    const ID: &str = "550e8400-e29b-41d4-a716-446655440000";

    /// 第1部 §3 分岐表の 16 行。行 17(`custom` + `cli_command == None`)は
    /// `resume_plan` の射程外であり、`build_launch_command` の Custom 腕へ委譲される
    /// (下の `branch_table_row_17_is_rejected_by_build_launch_command`)。
    #[test]
    fn covers_every_row_of_the_branch_table() {
        use CliKind::*;
        use FreshStartReason::*;
        use SessionMode::*;

        let cases: Vec<(u32, CliKind, Option<&str>, SessionMode, ResumePlan)> = vec![
            (
                1,
                Claude,
                Some(ID),
                Worktree,
                ResumePlan::ClaudeResume {
                    claude_session_id: ID.to_string(),
                },
            ),
            (
                2,
                Claude,
                Some(ID),
                InPlace,
                ResumePlan::ClaudeResume {
                    claude_session_id: ID.to_string(),
                },
            ),
            (3, Claude, None, Worktree, ResumePlan::ClaudeContinue),
            (
                4,
                Claude,
                None,
                InPlace,
                ResumePlan::FreshStart {
                    reason: AmbiguousInPlaceConversation,
                },
            ),
            (
                5,
                Codex,
                Some(ID),
                Worktree,
                ResumePlan::FreshStart {
                    reason: NoConversationRestore,
                },
            ),
            (
                6,
                Codex,
                Some(ID),
                InPlace,
                ResumePlan::FreshStart {
                    reason: NoConversationRestore,
                },
            ),
            (
                7,
                Codex,
                None,
                Worktree,
                ResumePlan::FreshStart {
                    reason: NoConversationRestore,
                },
            ),
            (
                8,
                Codex,
                None,
                InPlace,
                ResumePlan::FreshStart {
                    reason: NoConversationRestore,
                },
            ),
            (
                9,
                Shell,
                Some(ID),
                Worktree,
                ResumePlan::FreshStart {
                    reason: NoConversationRestore,
                },
            ),
            (
                10,
                Shell,
                Some(ID),
                InPlace,
                ResumePlan::FreshStart {
                    reason: NoConversationRestore,
                },
            ),
            (
                11,
                Shell,
                None,
                Worktree,
                ResumePlan::FreshStart {
                    reason: NoConversationRestore,
                },
            ),
            (
                12,
                Shell,
                None,
                InPlace,
                ResumePlan::FreshStart {
                    reason: NoConversationRestore,
                },
            ),
            (
                13,
                Custom,
                Some(ID),
                Worktree,
                ResumePlan::FreshStart {
                    reason: NoConversationRestore,
                },
            ),
            (
                14,
                Custom,
                Some(ID),
                InPlace,
                ResumePlan::FreshStart {
                    reason: NoConversationRestore,
                },
            ),
            (
                15,
                Custom,
                None,
                Worktree,
                ResumePlan::FreshStart {
                    reason: NoConversationRestore,
                },
            ),
            (
                16,
                Custom,
                None,
                InPlace,
                ResumePlan::FreshStart {
                    reason: NoConversationRestore,
                },
            ),
        ];

        assert_eq!(cases.len(), 16, "分岐表の行が欠けている");

        for (row, cli_kind, csid, mode, expected) in cases {
            let actual = resume_plan(&session(cli_kind, csid, mode));
            assert_eq!(actual, expected, "分岐表 行 {row} が一致しない");
        }
    }

    /// 第1部 §3 分岐表の 16 行を `ResumeMode` まで通した対応表。
    ///
    /// 上の `covers_every_row_of_the_branch_table` が `Session` -> `ResumePlan` を
    /// 固定するのに対し、こちらは `resume_plan()` -> `resume_mode()` の合成を固定する。
    /// `ResumeMode` から先(`program` / `args`)は `build_launch_command` の既存テスト群が
    /// 持っているので、ここでは列を持たない。
    #[test]
    fn resume_mode_covers_every_row_of_the_branch_table() {
        use CliKind::*;
        use SessionMode::*;

        let cases: Vec<(u32, CliKind, Option<&str>, SessionMode, ResumeMode<'_>)> = vec![
            (1, Claude, Some(ID), Worktree, ResumeMode::SessionId(ID)),
            (2, Claude, Some(ID), InPlace, ResumeMode::SessionId(ID)),
            (3, Claude, None, Worktree, ResumeMode::Continue),
            (4, Claude, None, InPlace, ResumeMode::None),
            (5, Codex, Some(ID), Worktree, ResumeMode::None),
            (6, Codex, Some(ID), InPlace, ResumeMode::None),
            (7, Codex, None, Worktree, ResumeMode::None),
            (8, Codex, None, InPlace, ResumeMode::None),
            (9, Shell, Some(ID), Worktree, ResumeMode::None),
            (10, Shell, Some(ID), InPlace, ResumeMode::None),
            (11, Shell, None, Worktree, ResumeMode::None),
            (12, Shell, None, InPlace, ResumeMode::None),
            (13, Custom, Some(ID), Worktree, ResumeMode::None),
            (14, Custom, Some(ID), InPlace, ResumeMode::None),
            (15, Custom, None, Worktree, ResumeMode::None),
            (16, Custom, None, InPlace, ResumeMode::None),
        ];

        assert_eq!(cases.len(), 16, "分岐表の行が欠けている");

        for (row, cli_kind, csid, mode, expected) in cases {
            let plan = resume_plan(&session(cli_kind, csid, mode));
            assert_eq!(resume_mode(&plan), expected, "分岐表 行 {row} が一致しない");
        }
    }

    /// 分岐表 行 5〜8。codex は resume フラグ体系が未実測(契約 §12.6 の表は claude の
    /// 実測しか持たない)なので、`build_launch_command` の
    /// `CliKind::Claude | CliKind::Codex` の腕へ非 `None` の `ResumeMode` が届かない
    /// ことを、`cli_kind` の分岐ではなく `resume_plan` の出力の側で固定する。
    #[test]
    fn codex_never_receives_a_non_none_resume_mode() {
        for (csid, mode) in [
            (Some(ID), SessionMode::Worktree),
            (Some(ID), SessionMode::InPlace),
            (None, SessionMode::Worktree),
            (None, SessionMode::InPlace),
        ] {
            let plan = resume_plan(&session(CliKind::Codex, csid, mode));
            assert_eq!(
                resume_mode(&plan),
                ResumeMode::None,
                "codex へ非 None の ResumeMode が渡る経路ができている ({csid:?} / {mode:?})"
            );
        }
    }

    /// 分岐表 行 17。`resume_plan` はここを扱わず、`build_launch_command` の Custom 腕へ
    /// 委譲する —— 委譲先が `AppError::InvalidState` を返すことをこのテストが固定する。
    #[test]
    fn branch_table_row_17_is_rejected_by_build_launch_command() {
        let mut target = session(CliKind::Custom, None, SessionMode::Worktree);
        target.cli_command = None;
        let plan = resume_plan(&target);

        let err = build_launch_command(
            &target,
            "/bin/zsh",
            Path::new("/work"),
            &LaunchEnv {
                path: "/fake/bin".to_string(),
                lang: "ja_JP.UTF-8".to_string(),
            },
            resume_mode(&plan),
        )
        .expect_err("行 17 は cli_command が無いので起動コマンドを組めない");
        assert!(matches!(err, AppError::InvalidState(_)), "got {err:?}");
    }

    #[test]
    fn in_place_claude_without_id_never_uses_continue() {
        let plan = resume_plan(&session(CliKind::Claude, None, SessionMode::InPlace));
        assert_ne!(
            plan,
            ResumePlan::ClaudeContinue,
            "in_place で --continue を選ぶと別セッションの会話に誤結合する"
        );
    }
}
