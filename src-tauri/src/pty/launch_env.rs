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
/// （呼び出し側の `probe_login_env_with` が `system_lang()` にフォールバックする）。
///
/// 生の探査結果のみを返す内部ヘルパ。フォールバック方針は `probe_login_env_with` が持つ。
/// タイムアウトを注入できる版に委譲する薄いラッパ（公開経路は `PROBE_TIMEOUT` を渡す）。
fn probe_raw(shell: &str) -> Option<(String, String)> {
    probe_raw_with_timeout(shell, PROBE_TIMEOUT)
}

/// タイムアウト値を注入できる版。テストが実測 `PROBE_TIMEOUT`（2 秒）を待たずに
/// 「タイムアウトしても panic せずフォールバックする」ことを検証できるようにするため
/// 分離した。フェーズ境界を越えて参照されないため契約 §60.4 によりモジュール内部に留める。
fn probe_raw_with_timeout(shell: &str, timeout: Duration) -> Option<(String, String)> {
    let script =
        format!(r#"printf "{PATH_BEGIN}%s{PATH_END}{LANG_BEGIN}%s{LANG_END}" "$PATH" "$LANG""#);
    let shell = shell.to_string();

    let (tx, rx) = mpsc::channel();
    // タイムアウト時にメインスレッドを待たせないよう別スレッドで実行し、recv_timeout で
    // 抜ける。タイムアウトした場合、この子スレッドと起動済みのシェルプロセスは即座には
    // 終了せず残り得る（kill していない）。ただし `probe_login_env()` は OnceLock で
    // プロセス全体につき高々 1 回しかこの経路を呼ばないため、漏れは最大 1 スレッド・
    // 1 子プロセスに有界であり、許容する。メインスレッドは子の回収を待たない。
    std::thread::spawn(move || {
        let result = Command::new(&shell)
            .args([SHELL_PROBE_FLAGS, &script])
            .output();
        let _ = tx.send(result);
    });

    let output = rx.recv_timeout(timeout).ok()?.ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    // PATH が取れなければ探査自体が失敗とみなす。LANG は空でも成功扱い
    // （GUI 起動では実際に空になるため。契約 §18）。
    let path = extract_between(&stdout, PATH_BEGIN, PATH_END)?;
    let lang = extract_between(&stdout, LANG_BEGIN, LANG_END).unwrap_or_default();
    Some((path, lang))
}

/// 探査に使うシェル起動を注入できる版。フォールバック（PATH は `env::var("PATH")` →
/// `MINIMAL_PATH`、LANG は空なら `system_lang()`）まで含めて必ず `LaunchEnv` を返す
/// （panic しない）。契約 §60.4 の注入版 seam。`probe_login_env` はここへ委譲する
/// 薄いラッパである。
pub fn probe_login_env_with(shell: &str) -> LaunchEnv {
    let probed = probe_raw(shell);

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
/// `defaults read -g AppleLocale` の生の出力を `normalize_locale` に渡すだけの薄い層。
/// フェーズ境界を越えて参照されないため契約 §60.4 によりモジュール内部に留める。
fn system_lang() -> String {
    let raw = Command::new("defaults")
        .args(["read", "-g", "AppleLocale"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    normalize_locale(&raw)
}

/// システムから得たロケール文字列を POSIX ロケール（`ll` または `ll_CC`）+ `.UTF-8` へ
/// 正規化する純関数。`defaults` の出力を注入できないテストのために分離した
/// （`system_lang` は macOS 実機の `defaults` コマンドに依存するため 1 入力しか踏めない）。
///
/// 手順:
/// 1. `@` 以降を落とす（`ja_JP@calendar=gregorian` のような修飾子）
/// 2. `-` を `_` に置き換える（`defaults` が BCP-47 のハイフン形を返す環境がある）
/// 3. `ll` / `ll_CC` の形として妥当でなければ `en_US.UTF-8` にフォールバックする
///    （`zh-Hans-CN` のような 3 要素は POSIX ロケールとして解釈できないため。
///    契約 §18 は「ハードコードよりシステム由来を優先せよ」という意味であり、
///    システム由来の値が POSIX として解釈不能な場合まで禁じてはいない）
/// 4. `.UTF-8` を付ける
fn normalize_locale(raw: &str) -> String {
    let base = raw.split('@').next().unwrap_or(raw).replace('-', "_");
    if is_posix_locale(&base) {
        format!("{base}.UTF-8")
    } else {
        "en_US.UTF-8".to_string()
    }
}

/// `ll` または `ll_CC`（`ll`/`CC` は英字のみ、2〜3 文字）の形かどうか。
fn is_posix_locale(s: &str) -> bool {
    match s.split('_').collect::<Vec<_>>().as_slice() {
        [language] => is_alpha_of_len(language, 2..=3),
        [language, country] => is_alpha_of_len(language, 2..=3) && is_alpha_of_len(country, 2..=2),
        _ => false,
    }
}

fn is_alpha_of_len(s: &str, len: std::ops::RangeInclusive<usize>) -> bool {
    len.contains(&s.chars().count()) && s.chars().all(|c| c.is_ascii_alphabetic())
}

/// `OnceLock` を注入できる版。テストが実ユーザーの `$SHELL` を起動せずに「探査は
/// 高々 1 回しか呼ばれない」ことを検証できるようにするため分離した
/// （キャッシュ機構そのものは `probe_login_env` と共有する）。
fn get_or_probe(cache: &OnceLock<LaunchEnv>, probe: impl FnOnce() -> LaunchEnv) -> &LaunchEnv {
    cache.get_or_init(probe)
}

/// 契約 §18: 1 回だけ探査してキャッシュする。失敗時もフォールバックし panic しない。
/// `probe_login_env_with` へ委譲する薄いラッパ（契約 §60.4）。
pub fn probe_login_env() -> &'static LaunchEnv {
    static CACHE: OnceLock<LaunchEnv> = OnceLock::new();
    get_or_probe(&CACHE, || probe_login_env_with(&user_shell()))
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
            LaunchEnv {
                path: "/fake/bin:/usr/bin".to_string(),
                lang: "ja_JP.UTF-8".to_string()
            }
        );
    }

    #[test]
    fn probe_falls_back_to_system_lang_when_probed_lang_is_empty() {
        // GUI 起動では LANG が空。契約 §18: そのとき system_lang() の値へフォールバックする
        // （空文字のまま渡すと nvim で日本語ファイル名が化けるため）。
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

        let env = probe_login_env_with(shell.to_str().expect("utf8"));
        assert_eq!(env.path, "/fake/bin");
        assert_eq!(env.lang, system_lang());
        assert!(!env.lang.is_empty());
    }

    #[test]
    fn probe_falls_back_to_process_path_when_shell_is_missing() {
        // レビュー指摘: 探査失敗時のフォールバック（契約 §18「失敗時は
        // std::env::var("PATH") にフォールバックする」）は、注入版に到達できないと
        // 検証不能だった。存在しないシェルを渡して確実に探査を失敗させる。
        let env = probe_login_env_with("/nonexistent/shell/xyz");
        assert_eq!(env.path, std::env::var("PATH").unwrap_or_default());
        assert_eq!(env.lang, system_lang());
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

        let _ = probe_login_env_with(shell.to_str().expect("utf8"));

        let recorded = std::fs::read_to_string(&args_file).expect("read args");
        assert_eq!(recorded, "-ilc");
    }

    #[test]
    fn probe_raw_with_timeout_falls_back_to_none_without_hanging() {
        // 契約 §0: unwrap による panic 経路も、永久待機も禁止。実測 PROBE_TIMEOUT（2 秒）を
        // 待つと CI の既定実行を遅くするので、タイムアウト値を短く注入して決定的かつ
        // 高速に検証する。値そのもの（2 秒）は起動予算を食わない設計（判断 4）なので変えない。
        let dir = tempfile::tempdir().expect("tempdir");
        let shell = dir.path().join("slowshell");
        std::fs::write(&shell, "#!/bin/sh\nsleep 5\n").expect("write");
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        let start = std::time::Instant::now();
        let result =
            probe_raw_with_timeout(shell.to_str().expect("utf8"), Duration::from_millis(50));
        let elapsed = start.elapsed();

        assert_eq!(result, None);
        assert!(
            elapsed < Duration::from_secs(1),
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

    // ---- normalize_locale（純関数。system_lang のハイフン形・不正形の正規化） ----

    #[test]
    fn normalize_locale_passes_through_underscore_form() {
        assert_eq!(normalize_locale("ja_JP"), "ja_JP.UTF-8");
    }

    #[test]
    fn normalize_locale_converts_hyphen_form_to_underscore() {
        // 判断 4 のレビュー指摘: defaults が BCP-47 のハイフン形（ja-JP）を返す環境がある
        assert_eq!(normalize_locale("ja-JP"), "ja_JP.UTF-8");
    }

    #[test]
    fn normalize_locale_falls_back_for_three_part_bcp47_form() {
        // zh_Hans_CN は POSIX ロケール（ll または ll_CC）として無効なのでフォールバックする
        assert_eq!(normalize_locale("zh-Hans-CN"), "en_US.UTF-8");
    }

    #[test]
    fn normalize_locale_strips_modifier_suffix() {
        assert_eq!(normalize_locale("en_US@calendar=japanese"), "en_US.UTF-8");
    }

    #[test]
    fn normalize_locale_falls_back_for_empty_input() {
        assert_eq!(normalize_locale(""), "en_US.UTF-8");
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
    fn get_or_probe_invokes_the_probe_at_most_once() {
        // 契約 §0: 起動 1 秒未満。310ms の副作用が毎回走ってはならない。
        // レビュー指摘: `probe_login_env()` を直接呼ぶと実ユーザーの $SHELL（と .zshrc）に
        // 依存し CI ランナーで再現しない上、std::ptr::eq だけでは「本当に 1 回しか
        // 呼ばれていないか」を検証できない（探査結果が何であれ緑になる）。
        // ローカルな OnceLock と呼び出し回数カウンタを注入し、実シェルなしで検証する。
        use std::sync::atomic::{AtomicUsize, Ordering};

        let cache = OnceLock::new();
        let calls = AtomicUsize::new(0);
        let probe = || {
            calls.fetch_add(1, Ordering::SeqCst);
            LaunchEnv {
                path: "/fake".to_string(),
                lang: "C".to_string(),
            }
        };

        let a = get_or_probe(&cache, probe);
        let b = get_or_probe(&cache, probe);

        assert!(std::ptr::eq(a, b));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
