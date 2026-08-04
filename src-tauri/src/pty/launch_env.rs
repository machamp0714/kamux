//! GUI 起動時の PATH 問題（契約 §18）。
//!
//! Finder / Dock から起動した macOS アプリは launchd 経由のため、ユーザーの
//! ログインシェルの PATH を継承しない。`claude` は `~/.local/bin` や nodenv
//! shims にあるのが典型なので、そのままでは常に未検出になる。
//!
//! **`-l -c` ではなく `-ilc` を使う理由**: zsh は `.zshrc` を**インタラクティブ時のみ**
//! 読む。nodenv / rbenv / nvm / mise / bun / 公式インストーラの PATH 追加は
//! ほぼ `.zshrc` に書かれるため、`-l -c` では取りこぼす（判断 4 に実測値）。
//!
//! 実測コスト ~310ms のため、`probe_login_env()` は初回呼び出し時に 1 度だけ
//! 解決して `OnceLock` にキャッシュする（契約 §0「起動 1 秒未満」）。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::Duration;

use crate::error::{AppError, AppResult};

/// インタラクティブシェルの出力ノイズと PATH を切り分ける目印。
const PATH_BEGIN: &str = "__KAMUX_PATH_BEGIN__";
const PATH_END: &str = "__KAMUX_PATH_END__";
const LANG_BEGIN: &str = "__KAMUX_LANG_BEGIN__";
const LANG_END: &str = "__KAMUX_LANG_END__";

/// シェルに渡すフラグ。契約 §18 の裁定により `-ilc` に確定している。
const SHELL_PROBE_FLAGS: &str = "-ilc";

/// シェル probe のタイムアウト。超えたらフォールバックする。
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

const FALLBACK_ABSOLUTE_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];
const FALLBACK_HOME_RELATIVE_DIRS: &[&str] = &[".claude/local", ".local/bin"];

/// probe が完全に失敗したときの最後の砦。
const MINIMAL_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

const INSTALL_DOCS_URL: &str = "https://docs.claude.com/en/docs/claude-code/setup";

/// CLI を直接 exec する経路に注入する環境（契約 §18）。
/// M1-4 / M3-1 がこのファイルに `probe_login_env()`（`$SHELL -ilc` による探査）と
/// `resolve_program()` を追加する。M1-3 は `$SHELL -l` しか起動しないため探査は不要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchEnv {
    /// ログインシェルから取得した PATH
    pub path: String,
    /// ログインシェルから取得した LANG。空だと nvim で日本語ファイル名が化ける
    pub lang: String,
}

impl LaunchEnv {
    /// 現プロセスの環境をそのまま使う暫定コンストラクタ。
    /// GUI 起動では PATH が不完全になるため、M1-4 が `probe_login_env()` に差し替える。
    pub fn from_current_process() -> Self {
        Self {
            path: std::env::var("PATH").unwrap_or_default(),
            lang: std::env::var("LANG").unwrap_or_default(),
        }
    }
}

/// センチネルに挟まれた値を取り出す。見つからない/空なら None。
/// フェーズ境界を越えて参照されないため契約 §60.4 によりモジュール内部に留める。
fn extract_between(stdout: &str, begin: &str, end: &str) -> Option<String> {
    let start = stdout.find(begin)? + begin.len();
    let rest = &stdout[start..];
    let stop = rest.find(end)?;
    let value = &rest[..stop];
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// 指定シェルをログイン+インタラクティブで**1 回だけ**起動し、PATH と LANG を同時に取る。
/// タイムアウト・起動失敗・パース失敗はすべて None（panic しない）。
///
/// **1 回にまとめる理由**: `-ilc` は 1 回あたり約 310ms かかる（契約 §18 の実測値）。
/// PATH と LANG で 2 回起動すると 620ms となり、契約 §0 の「起動 1 秒未満」を
/// 単独で圧迫する。LANG が空文字のときは None ではなく空文字が返る点に注意
/// （呼び出し側の `probe_login_env` が `system_lang()` にフォールバックする）。
///
/// 契約 §60.4 の注入版 seam。`probe_login_env` はここへ委譲する薄いラッパである。
pub fn probe_login_env_with(shell: &str) -> Option<(String, String)> {
    let script =
        format!(r#"printf "{PATH_BEGIN}%s{PATH_END}{LANG_BEGIN}%s{LANG_END}" "$PATH" "$LANG""#);
    let shell = shell.to_string();

    let (tx, rx) = mpsc::channel();
    // タイムアウト時に UI を待たせないよう別スレッドで待つ。
    // タイムアウトしてもシェルは自然終了するのでスレッドは回収される。
    std::thread::spawn(move || {
        let result = Command::new(&shell)
            .args([SHELL_PROBE_FLAGS, &script])
            .output();
        let _ = tx.send(result);
    });

    let output = rx.recv_timeout(PROBE_TIMEOUT).ok()?.ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    // PATH が取れなければ探査自体が失敗とみなす。LANG は空でも成功扱い
    // （GUI 起動では実際に空になるため。契約 §18）。
    let path = extract_between(&stdout, PATH_BEGIN, PATH_END)?;
    let lang = extract_between(&stdout, LANG_BEGIN, LANG_END).unwrap_or_default();
    Some((path, lang))
}

/// 既知の CLI インストール先候補。
/// フェーズ境界を越えて参照されないため契約 §60.4 によりモジュール内部に留める。
fn fallback_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        for rel in FALLBACK_HOME_RELATIVE_DIRS {
            dirs.push(home.join(rel));
        }
    }
    for abs in FALLBACK_ABSOLUTE_DIRS {
        dirs.push(PathBuf::from(abs));
    }
    dirs
}

fn user_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
}

/// macOS のシステムロケールから LANG を導出する（契約 §18 のフォールバック）。
/// `defaults read -g AppleLocale` は `ja_JP` のような値を返すので `.UTF-8` を付ける。
/// フェーズ境界を越えて参照されないため契約 §60.4 によりモジュール内部に留める。
fn system_lang() -> String {
    let out = Command::new("defaults")
        .args(["read", "-g", "AppleLocale"])
        .output()
        .ok()
        .filter(|o| o.status.success());

    match out {
        Some(o) => {
            let locale = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if locale.is_empty() {
                "en_US.UTF-8".to_string()
            } else {
                // "ja_JP@calendar=gregorian" のような修飾子を落とす
                let base = locale.split('@').next().unwrap_or(&locale);
                format!("{base}.UTF-8")
            }
        }
        None => "en_US.UTF-8".to_string(),
    }
}

/// 契約 §18: 1 回だけ探査してキャッシュする。失敗時もフォールバックし panic しない。
pub fn probe_login_env() -> &'static LaunchEnv {
    static CACHE: OnceLock<LaunchEnv> = OnceLock::new();
    CACHE.get_or_init(|| {
        let probed = probe_login_env_with(&user_shell());

        let path = probed
            .as_ref()
            .map(|(p, _)| p.clone())
            .or_else(|| std::env::var("PATH").ok())
            .unwrap_or_else(|| MINIMAL_PATH.to_string());

        // GUI 起動では LANG が空になるのでシステムロケールから導出する（契約 §18）
        let lang = probed
            .and_then(|(_, l)| if l.is_empty() { None } else { Some(l) })
            .unwrap_or_else(system_lang);

        LaunchEnv { path, lang }
    })
}

/// 実行可能な「ファイル」かどうか。同名ディレクトリを弾く。
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && meta.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

/// 検索パスを注入できる版。テストが実行環境のインストール状態に依存しないようにする。
/// 契約 §60.4 の注入版 seam。`resolve_program` はここへ委譲する薄いラッパである。
pub fn resolve_program_in(
    program: &str,
    search_path: &str,
    extra_dirs: &[PathBuf],
) -> AppResult<PathBuf> {
    let path_dirs: Vec<PathBuf> = search_path
        .split(':')
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect();

    for dir in path_dirs.iter().chain(extra_dirs.iter()) {
        let candidate = dir.join(program);
        if is_executable_file(&candidate) {
            return Ok(candidate);
        }
    }

    let searched: Vec<String> = path_dirs
        .iter()
        .chain(extra_dirs.iter())
        .map(|d| d.display().to_string())
        .collect();

    Err(AppError::CliNotFound(format!(
        "`{program}` が見つかりませんでした。\n\
         検索したディレクトリ:\n  {}\n\n\
         インストール手順: {INSTALL_DOCS_URL}\n\
         すでにインストール済みの場合は、ターミナルで `which {program}` を実行し、\n\
         表示されたディレクトリがログインシェル（~/.zshrc など）の PATH に\n\
         含まれているか確認してください。",
        searched.join("\n  ")
    )))
}

/// 契約 §18 の公開 API。探査済み PATH とフォールバック候補で解決する。
pub fn resolve_program(program: &str) -> AppResult<PathBuf> {
    resolve_program_in(program, &probe_login_env().path, &fallback_dirs())
}

// env の組み立ては契約 §23 の `build_launch_command` に集約する（Task 7）。
// このモジュールは「探査」と「解決」だけを担い、env を組まない。

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    // PATH は cargo test の実行環境で必ず非空なので、path/lang の取り違え変異を
    // 環境変数の書き換えなしに検出できる（並列テストと干渉しない）。
    #[test]
    fn from_current_process_reads_path_into_the_path_field_not_lang() {
        let env = LaunchEnv::from_current_process();
        assert_eq!(env.path, std::env::var("PATH").unwrap_or_default());
    }

    #[test]
    fn from_current_process_reads_lang_into_the_lang_field_not_path() {
        let env = LaunchEnv::from_current_process();
        assert_eq!(env.lang, std::env::var("LANG").unwrap_or_default());
    }

    /// 実行可能ファイルを作る。実 claude に依存せず探索を検証するため。
    fn make_exe(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, "#!/bin/sh\nexit 0\n").expect("write");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        p
    }

    // ---- センチネル抽出 ----

    #[test]
    fn extracts_value_between_sentinels() {
        let out = format!("{PATH_BEGIN}/usr/bin:/bin{PATH_END}");
        assert_eq!(
            extract_between(&out, PATH_BEGIN, PATH_END),
            Some("/usr/bin:/bin".to_string())
        );
    }

    #[test]
    fn extracts_despite_interactive_shell_noise() {
        // -i を付けるとシェルが起動メッセージやエスケープを吐きうる（判断 4）
        let out = format!(
            "\u{1b}]0;title\u{7}welcome\n{PATH_BEGIN}/opt/homebrew/bin:/usr/bin{PATH_END}\nbye\n"
        );
        assert_eq!(
            extract_between(&out, PATH_BEGIN, PATH_END),
            Some("/opt/homebrew/bin:/usr/bin".to_string())
        );
    }

    #[test]
    fn returns_none_without_sentinels() {
        assert_eq!(extract_between("just noise", PATH_BEGIN, PATH_END), None);
        assert_eq!(
            extract_between(&format!("{PATH_BEGIN}unterminated"), PATH_BEGIN, PATH_END),
            None
        );
    }

    #[test]
    fn returns_none_for_empty_value() {
        assert_eq!(
            extract_between(&format!("{PATH_BEGIN}{PATH_END}"), PATH_BEGIN, PATH_END),
            None
        );
    }

    // ---- シェル probe（1 回で PATH と LANG 両方） ----

    #[test]
    fn probes_path_and_lang_in_a_single_shell_invocation() {
        // 実 zsh に依存せず、引数を無視して両センチネルを吐く偽シェルで検証する
        let dir = tempfile::tempdir().expect("tempdir");
        let shell = dir.path().join("fakeshell");
        std::fs::write(
            &shell,
            format!(
                "#!/bin/sh\nprintf '{PATH_BEGIN}%s{PATH_END}{LANG_BEGIN}%s{LANG_END}' \
                 /fake/bin:/usr/bin ja_JP.UTF-8\n"
            ),
        )
        .expect("write fake shell");
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        assert_eq!(
            probe_login_env_with(shell.to_str().expect("utf8")),
            Some(("/fake/bin:/usr/bin".to_string(), "ja_JP.UTF-8".to_string()))
        );
    }

    #[test]
    fn probe_succeeds_with_empty_lang() {
        // GUI 起動では LANG が空。PATH さえ取れれば探査は成功扱いにする
        let dir = tempfile::tempdir().expect("tempdir");
        let shell = dir.path().join("fakeshell");
        std::fs::write(
            &shell,
            format!(
                "#!/bin/sh\nprintf '{PATH_BEGIN}%s{PATH_END}{LANG_BEGIN}{LANG_END}' /fake/bin\n"
            ),
        )
        .expect("write");
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        assert_eq!(
            probe_login_env_with(shell.to_str().expect("utf8")),
            Some(("/fake/bin".to_string(), String::new()))
        );
    }

    #[test]
    fn probe_returns_none_for_missing_shell() {
        assert_eq!(probe_login_env_with("/nonexistent/shell/xyz"), None);
    }

    #[test]
    fn probe_invokes_shell_with_ilc_flags() {
        // 判断 4: `-l -c` ではなく `-ilc` でなければ .zshrc（nodenv/nvm/公式インストーラの
        // PATH 追加が書かれる場所）を読まない。実行時に渡された第一引数を記録して検証する。
        let dir = tempfile::tempdir().expect("tempdir");
        let shell = dir.path().join("fakeshell");
        let args_file = dir.path().join("args.txt");
        std::fs::write(
            &shell,
            format!(
                "#!/bin/sh\nprintf '%s' \"$1\" > '{}'\n\
                 printf '{PATH_BEGIN}%s{PATH_END}{LANG_BEGIN}%s{LANG_END}' /fake/bin C\n",
                args_file.display()
            ),
        )
        .expect("write fake shell");
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        probe_login_env_with(shell.to_str().expect("utf8"));

        let recorded = std::fs::read_to_string(&args_file).expect("read args");
        assert_eq!(recorded, "-ilc");
    }

    #[test]
    fn probe_login_env_with_times_out_instead_of_hanging() {
        // 契約 §0: unwrap による panic 経路も、永久待機も禁止。タイムアウトで
        // None に落ちることと、実測時間が PROBE_TIMEOUT を大きく超えないことを確認する。
        let dir = tempfile::tempdir().expect("tempdir");
        let shell = dir.path().join("slowshell");
        std::fs::write(&shell, "#!/bin/sh\nsleep 5\n").expect("write");
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        let start = std::time::Instant::now();
        let result = probe_login_env_with(shell.to_str().expect("utf8"));
        let elapsed = start.elapsed();

        assert_eq!(result, None);
        assert!(
            elapsed < PROBE_TIMEOUT + Duration::from_secs(1),
            "took too long: {elapsed:?}"
        );
    }

    #[test]
    fn system_lang_is_a_utf8_locale() {
        // 契約 §18: 空の LANG で nvim を起動すると日本語ファイル名が化ける
        let lang = system_lang();
        assert!(lang.ends_with(".UTF-8"), "got {lang}");
        assert!(
            !lang.starts_with('.'),
            "locale part must not be empty: {lang}"
        );
        assert!(!lang.contains('@'), "modifiers must be stripped: {lang}");
    }

    // ---- resolve_program_in（注入版） ----

    #[test]
    fn finds_program_on_search_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let expected = make_exe(dir.path(), "claude");
        let found =
            resolve_program_in("claude", dir.path().to_str().expect("utf8"), &[]).expect("resolve");
        assert_eq!(found, expected);
    }

    #[test]
    fn honours_search_path_order() {
        let first = tempfile::tempdir().expect("tempdir");
        let second = tempfile::tempdir().expect("tempdir");
        let expected = make_exe(first.path(), "claude");
        make_exe(second.path(), "claude");

        let path = format!(
            "{}:{}",
            first.path().to_str().expect("utf8"),
            second.path().to_str().expect("utf8")
        );
        assert_eq!(
            resolve_program_in("claude", &path, &[]).expect("resolve"),
            expected
        );
    }

    #[test]
    fn falls_back_to_extra_dirs_when_not_on_path() {
        let path_dir = tempfile::tempdir().expect("tempdir");
        let extra_dir = tempfile::tempdir().expect("tempdir");
        let expected = make_exe(extra_dir.path(), "claude");

        let found = resolve_program_in(
            "claude",
            path_dir.path().to_str().expect("utf8"),
            &[extra_dir.path().to_path_buf()],
        )
        .expect("resolve");
        assert_eq!(found, expected);
    }

    #[test]
    fn ignores_non_executable_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("claude");
        std::fs::write(&p, "not executable").expect("write");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).expect("chmod");

        let err =
            resolve_program_in("claude", dir.path().to_str().expect("utf8"), &[]).unwrap_err();
        assert!(matches!(err, AppError::CliNotFound(_)));
    }

    #[test]
    fn ignores_directories_named_like_the_program() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("claude")).expect("mkdir");

        let err =
            resolve_program_in("claude", dir.path().to_str().expect("utf8"), &[]).unwrap_err();
        assert!(matches!(err, AppError::CliNotFound(_)));
    }

    #[test]
    fn missing_program_error_includes_actionable_guidance() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err =
            resolve_program_in("claude", dir.path().to_str().expect("utf8"), &[]).unwrap_err();

        match err {
            AppError::CliNotFound(msg) => {
                assert!(msg.contains("claude"), "must name the binary: {msg}");
                assert!(
                    msg.contains("which claude"),
                    "must tell how to diagnose: {msg}"
                );
                assert!(msg.contains("https://"), "must link install docs: {msg}");
                assert!(
                    msg.contains(dir.path().to_str().expect("utf8")),
                    "must list searched dirs: {msg}"
                );
            }
            other => panic!("expected CliNotFound, got {other:?}"),
        }
    }

    // ---- env 組立 ----

    #[test]
    fn fallback_dirs_include_known_claude_locations() {
        let dirs = fallback_dirs();
        let as_str: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
        assert!(
            as_str.iter().any(|d| d.ends_with("/.local/bin")),
            "{as_str:?}"
        );
        assert!(
            as_str.iter().any(|d| d.ends_with("/.claude/local")),
            "{as_str:?}"
        );
        assert!(
            as_str.iter().any(|d| d == "/opt/homebrew/bin"),
            "{as_str:?}"
        );
        assert!(as_str.iter().any(|d| d == "/usr/local/bin"), "{as_str:?}");
    }

    // ---- probe_login_env（キャッシュ） ----

    #[test]
    fn probe_login_env_caches_result_across_calls() {
        // 契約 §0: 起動 1 秒未満。310ms の副作用が毎回走ってはならない。
        // OnceLock により 2 回目以降は同一インスタンスを返すことで再探査していないと確認する。
        let a = probe_login_env();
        let b = probe_login_env();
        assert!(std::ptr::eq(a, b));
    }
}
