//! kamux が書き出す CLI shim（契約 §30.1 / §30.2 / §64.3）。
//!
//! スクラッチシェルでユーザーが**手で打った** `claude` にも `--settings` を届けるための
//! 仕組みである。`cli_args.rs` が argv を組むときにしか `--settings` は付かないので、
//! 手打ちでは hooks が飛ばず、入力待ち通知も `claude_session_id` の捕捉も効かない。
//!
//! **`~/Library/Application Support/kamux/shim/{claude,codex}` へアプリ起動のたびに
//! 書き出す**（毎回上書き。契約 §30.1 の表）。

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};
use crate::model::CliKind;

/// shim を置くサブディレクトリ名（契約 §30.1 の `…/kamux/shim/{claude,codex}`）。
pub const SHIM_DIR_NAME: &str = "shim";

/// 書き出す shim と、`--settings` を足すかどうか。
///
/// **`codex` の shim は `--settings` を足さない。** 契約 §30 の本文は逐語で
/// 「`--settings` は `cli_args.rs` が claude の argv を組むときだけ足す」と書いており
/// （`apply_hooks` の実装も `cli_kind == Claude` 限定になっている）、§30.1 のガードの
/// 目的は逐語で「存在しない settings ファイルを指す `--settings` を渡して claude を
/// 壊さないため」である —— codex に claude 専用フラグを渡すのは、そのガードが防ごうと
/// している事故そのものになる。**2 ファイルとも作るのは §30.1 の表が置き場所を
/// `shim/{claude,codex}` と定めているためである。**
pub const SHIMMED_CLIS: [(&str, bool); 2] = [("claude", true), ("codex", false)];

/// shim スクリプトの本体。`@BINARY@` と `@SETTINGS_BRANCH@` を差し替えて使う。
///
/// **本体の解決は `KAMUX_SHIM_DIR` を除いた PATH で行う**（自分自身を再帰的に exec
/// しないため。契約 §30.1）。`PATH=… command -v` という前置代入は `command` が
/// POSIX の**通常**組み込みであるため一時的にしか効かず、exec される本体には
/// shim ディレクトリを含んだ元の `PATH` がそのまま渡る。
const SHIM_TEMPLATE: &str = r#"#!/bin/sh
# kamux CLI shim（契約 §30.1）。アプリ起動のたびに書き直されるので手で編集しない。
#
# 本体の解決は KAMUX_SHIM_DIR を除いた PATH で行う（自分自身を再帰的に exec しないため）。
# KAMUX_SHIM_DIR が未設定でも自己再帰しないよう、$0 から導いた自分自身のディレクトリも
# 併せて除外する（契約 §154.2。KAMUX_SHIM_DIR の比較を置き換えるのではなく加える）。
# $0 に / が無い場合は導けないので除外候補にしない（契約 §154.3 ハザード 1）。
case "$0" in
  */*) kamux_self_dir=${0%/*} ;;
  *) kamux_self_dir='' ;;
esac
kamux_path=''
IFS=':'
for kamux_dir in $PATH; do
  [ "$kamux_dir" = "$KAMUX_SHIM_DIR" ] && continue
  [ -n "$kamux_self_dir" ] && [ "$kamux_dir" = "$kamux_self_dir" ] && continue
  if [ -z "$kamux_path" ]; then
    kamux_path="$kamux_dir"
  else
    kamux_path="$kamux_path:$kamux_dir"
  fi
done
unset IFS
kamux_real=$(PATH="$kamux_path" command -v @BINARY@) || {
  printf 'kamux shim: @BINARY@ not found in PATH\n' >&2
  exit 127
}
@SETTINGS_BRANCH@exec "$kamux_real" "$@"
"#;

/// `--settings` を足す腕（`claude` だけ）。
///
/// **`KAMUX_HOOKS_SETTINGS` が設定されているときだけ足す**（契約 §30.1）。shim
/// ディレクトリがユーザーの rc に残った場合や kamux 外のプロセスが拾った場合に、
/// 存在しない settings を指して claude を壊さないため。
const SETTINGS_BRANCH: &str = r#"# KAMUX_HOOKS_SETTINGS が設定されているときだけ --settings を足す（契約 §30.1）。
# 設定されていなければ引数を一切足さずに本体を exec する。
if [ -n "$KAMUX_HOOKS_SETTINGS" ]; then
  exec "$kamux_real" --settings "$KAMUX_HOOKS_SETTINGS" "$@"
fi
"#;

/// 契約 §64.3 の逐語（**書き換え不可**）。`cli_kind == Shell` の PTY へ spawn の成功
/// 直後に送る 1 行である。
///
/// **`[ -n … ]` のガードを外してはならない。** 素の `export PATH="$KAMUX_SHIM_DIR:$PATH"`
/// は、変数が未設定のとき `PATH` の先頭に空要素を作る。空要素は POSIX ではカレント
/// ディレクトリを意味し、`cwd` の実行ファイルを拾う経路になる。
pub const SHELL_PATH_LINE: &[u8] =
    b"[ -n \"$KAMUX_SHIM_DIR\" ] && export PATH=\"$KAMUX_SHIM_DIR:$PATH\"\n";

/// shim スクリプトの中身を組み立てる。
pub fn shim_script(binary: &str, adds_settings: bool) -> String {
    let settings_branch = if adds_settings { SETTINGS_BRANCH } else { "" };
    SHIM_TEMPLATE
        .replace("@SETTINGS_BRANCH@", settings_branch)
        .replace("@BINARY@", binary)
}

/// `base_dir/shim/` を作り、`SHIMMED_CLIS` の全 shim を実行可能な形で書き出す。
/// 戻り値は shim ディレクトリの絶対パス（= `HooksRuntime::shim_dir` に載る値）。
///
/// 毎回上書きする（契約 §30.1 の「生成」の行。バージョン差分を気にしない）。
pub fn install_shims(base_dir: &Path) -> AppResult<PathBuf> {
    let dir = base_dir.join(SHIM_DIR_NAME);
    std::fs::create_dir_all(&dir).map_err(|e| {
        AppError::Io(format!(
            "failed to create shim directory {}: {e}",
            dir.display()
        ))
    })?;

    for (binary, adds_settings) in SHIMMED_CLIS {
        let path = dir.join(binary);
        std::fs::write(&path, shim_script(binary, adds_settings))
            .map_err(|e| AppError::Io(format!("failed to write shim {}: {e}", path.display())))?;
        // 実行権限が落ちていると shim は PATH に居ても一度も走らない。
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| AppError::Io(format!("failed to chmod shim {}: {e}", path.display())))?;
    }
    Ok(dir)
}

/// 書き出しに失敗しても shim を無効にするだけでアプリは起動する
/// （設計 §12 の hooks と同じ fail-soft）。`None` は「shim 無効」であり、
/// `HooksRuntime::shim_dir` がそのまま `None` になる。
pub fn install_shims_or_none(base_dir: Option<&Path>) -> Option<PathBuf> {
    let base_dir = base_dir?;
    match install_shims(base_dir) {
        Ok(dir) => {
            tracing::info!(shim_dir = %dir.display(), "cli shims installed");
            Some(dir)
        }
        Err(e) => {
            tracing::warn!(error = %e, "shim disabled: failed to install cli shims");
            None
        }
    }
}

/// 契約 §30.1 の置き場所の親（`~/Library/Application Support/kamux`）。
/// `store::db_path()` と同じディレクトリである（契約 §0）—— Tauri の `app_data_dir()`
/// はバンドル identifier を含むパスを返すので使わない。
pub fn shim_base_dir() -> AppResult<PathBuf> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| AppError::Io("HOME environment variable is not set".to_owned()))?;
    Ok(PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("kamux"))
}

/// spawn 直後に PTY へ送る 1 行（契約 §64.3）。送るのは **shim 有効時かつ
/// `cli_kind == Shell` のときだけ**である。
pub fn shell_path_line(cli_kind: CliKind, shim_dir: Option<&Path>) -> Option<&'static [u8]> {
    match (cli_kind, shim_dir) {
        (CliKind::Shell, Some(_)) => Some(SHELL_PATH_LINE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 契約 §30.1 の逐語: 「shim は `KAMUX_HOOKS_SETTINGS` 環境変数が設定されている
    /// ときだけ `--settings` を足す。設定されていなければ、引数を一切足さずに本体を
    /// exec する。」
    ///
    /// ガードごと落として常に `--settings` を足す変異は、`if [ -n …` の行が消えるので
    /// ここで赤くなる。
    #[test]
    fn claude_shim_adds_settings_only_when_the_hooks_settings_env_is_set() {
        let script = shim_script("claude", true);

        assert!(
            script.contains("if [ -n \"$KAMUX_HOOKS_SETTINGS\" ]; then\n  exec \"$kamux_real\" --settings \"$KAMUX_HOOKS_SETTINGS\" \"$@\"\nfi\n"),
            "ガード付きの --settings 分岐が逐語で入っていない:\n{script}"
        );
        // 分岐の**外**に、引数を一切足さない exec がある（= env 未設定なら素通し）。
        assert!(
            script.ends_with("exec \"$kamux_real\" \"$@\"\n"),
            "引数を足さない exec が末尾に無い:\n{script}"
        );
    }

    /// 裁定 21-A: `codex` の shim は引数を一切変えずに本体を exec する。
    /// 契約 §30 の本文が逐語で「`--settings` は `cli_args.rs` が claude の argv を
    /// 組むときだけ足す」と書いている。
    #[test]
    fn codex_shim_never_adds_the_settings_flag() {
        let script = shim_script("codex", false);

        assert!(
            !script.contains("--settings"),
            "codex の shim に claude 専用フラグが入っている:\n{script}"
        );
        assert!(
            !script.contains("KAMUX_HOOKS_SETTINGS"),
            "codex の shim が settings env を見ている:\n{script}"
        );
        assert!(
            script.ends_with("exec \"$kamux_real\" \"$@\"\n"),
            "{script}"
        );
    }

    /// 契約 §30.1 の逐語: 「本体の解決は `KAMUX_SHIM_DIR` を除いた PATH で行う
    /// （自分自身を再帰的に exec しないため）。」
    #[test]
    fn shim_resolves_the_real_binary_from_a_path_without_the_shim_dir() {
        let script = shim_script("claude", true);

        // shim ディレクトリを PATH から落とす行。
        assert!(
            script.contains("[ \"$kamux_dir\" = \"$KAMUX_SHIM_DIR\" ] && continue\n"),
            "PATH から KAMUX_SHIM_DIR を除く行が無い:\n{script}"
        );
        // 本体の解決はその除いた PATH で行う。
        assert!(
            script.contains("kamux_real=$(PATH=\"$kamux_path\" command -v claude)"),
            "除いた PATH で本体を解決していない:\n{script}"
        );
        // 素の `exec claude` に戻す変異（= 自分自身を再帰 exec する）を弁別する。
        assert!(
            !script.contains("exec claude"),
            "本体を PATH 解決に任せている（自分自身を再帰 exec する）:\n{script}"
        );
    }

    /// 契約 §154.2: 自己除外を `KAMUX_SHIM_DIR` の設定に依存させない。`$0` から
    /// 導いた自分自身のディレクトリも PATH 走査の除外候補に加える（`KAMUX_SHIM_DIR`
    /// の比較と**併せて**。既存の比較を置き換えるのではない —— §30.1 の字面の担保は
    /// `shim_resolves_the_real_binary_from_a_path_without_the_shim_dir` が別に見ている）。
    #[test]
    fn shim_also_excludes_its_own_directory_derived_from_dollar_zero() {
        let script = shim_script("claude", true);

        // `$0` に `/` が無い場合（契約 §154.3 ハザード 1）に備えて分岐すること。
        assert!(
            script.contains("case \"$0\" in"),
            "$0 の形で分岐していない（ハザード 1 未対応の疑い）:\n{script}"
        );
        // 導いた自分自身のディレクトリも PATH 走査で除外する。
        assert!(
            script.contains("kamux_self_dir"),
            "$0 から導いた自分自身のディレクトリを除外候補にしていない:\n{script}"
        );
        // 既存の KAMUX_SHIM_DIR 比較は残る（置き換えではなく併用）。
        assert!(
            script.contains("[ \"$kamux_dir\" = \"$KAMUX_SHIM_DIR\" ] && continue\n"),
            "既存の KAMUX_SHIM_DIR 比較が失われている:\n{script}"
        );
    }

    #[test]
    fn shim_script_is_a_posix_sh_script() {
        for (binary, adds_settings) in SHIMMED_CLIS {
            let script = shim_script(binary, adds_settings);
            assert!(script.starts_with("#!/bin/sh\n"), "{binary}: {script}");
        }
    }

    /// 契約 §30.1 の表: 置き場所は `…/kamux/shim/{claude,codex}`。
    /// **実行権限が落ちていると shim は完全に死ぬ**ので、内容とは別に mode も見る。
    #[test]
    fn install_shims_writes_both_clis_as_executables_under_the_shim_dir() {
        use std::os::unix::fs::PermissionsExt;

        let base = tempfile::tempdir().expect("tempdir");
        let dir = install_shims(base.path()).expect("install shims");

        assert_eq!(dir, base.path().join("shim"));
        for (binary, adds_settings) in SHIMMED_CLIS {
            let path = dir.join(binary);
            let body = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            assert_eq!(body, shim_script(binary, adds_settings), "{binary}");

            let mode = std::fs::metadata(&path)
                .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()))
                .permissions()
                .mode();
            assert_ne!(mode & 0o111, 0, "{binary} に実行権限が無い: {mode:o}");
        }
    }

    /// 毎回上書きする（契約 §30.1 の「生成」の行）。前回の残骸が残らないこと。
    #[test]
    fn install_shims_overwrites_a_previous_generation() {
        let base = tempfile::tempdir().expect("tempdir");
        let dir = install_shims(base.path()).expect("first install");
        std::fs::write(dir.join("claude"), b"stale\n").expect("write stale");

        install_shims(base.path()).expect("second install");

        let body = std::fs::read_to_string(dir.join("claude")).expect("read");
        assert_eq!(body, shim_script("claude", true));
    }

    /// 書き出しに失敗しても shim を無効にするだけでアプリは起動する
    /// （設計 §12 の hooks と同じ fail-soft）。
    #[test]
    fn install_shims_or_none_is_none_without_a_base_dir() {
        assert_eq!(install_shims_or_none(None), None);
    }

    #[test]
    fn install_shims_or_none_is_none_when_the_base_dir_cannot_be_created() {
        // 既存の**ファイル**の下にはディレクトリを作れない。
        let base = tempfile::tempdir().expect("tempdir");
        let file = base.path().join("not-a-dir");
        std::fs::write(&file, b"x").expect("write");

        assert_eq!(install_shims_or_none(Some(&file)), None);
    }

    #[test]
    fn install_shims_or_none_returns_the_shim_dir_on_success() {
        let base = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            install_shims_or_none(Some(base.path())),
            Some(base.path().join("shim"))
        );
    }

    /// 契約 §30.1 の置き場所（`~/Library/Application Support/kamux/shim`）の親。
    /// `store::db_path()` と同じディレクトリである（契約 §0）。
    #[test]
    fn shim_base_dir_is_the_kamux_application_support_dir() {
        let base = shim_base_dir().expect("base dir");
        assert!(
            base.ends_with("Library/Application Support/kamux"),
            "actual: {}",
            base.display()
        );
        assert_eq!(
            base.join(SHIM_DIR_NAME),
            base.join("shim"),
            "契約 §30.1 のサブディレクトリ名"
        );
    }

    /// 生成した shim を **`/bin/sh` に実際に食わせる**。逐語の一致だけを見ていると、
    /// 文字列としては正しくシェル構文としては壊れている形（`if` の閉じ忘れ等）を
    /// 素通しする。実 `claude` には触れない —— argv をそのまま出す偽物を tempdir に
    /// 置き、`PATH` で注入する（契約 §14）。
    ///
    /// `KAMUX_SHIM_DIR` を PATH に**先頭で**入れたうえで走らせるので、shim が自分自身を
    /// 再帰 exec する形（契約 §30.1 の禁止）ならここでハングまたは 127 になる。
    fn run_shim(binary: &str, settings: Option<&str>) -> String {
        let base = tempfile::tempdir().expect("tempdir");
        let dir = install_shims(base.path()).expect("install shims");
        let real_dir = base.path().join("real");
        std::fs::create_dir_all(&real_dir).expect("mkdir real");
        let real = real_dir.join(binary);
        std::fs::write(&real, "#!/bin/sh\nprintf 'ARGS:%s\\n' \"$*\"\n").expect("write real");
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        let mut cmd = std::process::Command::new(dir.join(binary));
        cmd.arg("hello")
            .env("PATH", format!("{}:{}", dir.display(), real_dir.display()))
            .env("KAMUX_SHIM_DIR", &dir);
        match settings {
            Some(path) => cmd.env("KAMUX_HOOKS_SETTINGS", path),
            None => cmd.env_remove("KAMUX_HOOKS_SETTINGS"),
        };
        let out = cmd.output().expect("run shim");
        assert!(
            out.status.success(),
            "shim が失敗した: status={:?} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// 契約 §154.3: 自己再帰が再発したとき、このテストは「失敗」ではなく
    /// 「無限にハング」する。素の `Command::output()` にはタイムアウトが無いので、
    /// `spawn` + `try_wait` のポーリングで上限（10 秒）を超えたら `kill()` してから
    /// `panic!` する。既存の `run_shim`（`cmd.output()` ベース）は変更しない。
    fn run_shim_with_deadline(cmd: &mut std::process::Command) -> std::process::Output {
        use std::io::Read;
        use std::time::{Duration, Instant};

        let mut child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn shim");

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().expect("try_wait") {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut out) = child.stdout.take() {
                    out.read_to_end(&mut stdout).expect("read stdout");
                }
                if let Some(mut err) = child.stderr.take() {
                    err.read_to_end(&mut stderr).expect("read stderr");
                }
                return std::process::Output {
                    status,
                    stdout,
                    stderr,
                };
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("shim が 10 秒以内に終了しなかった。自己再帰 exec が疑われる（契約 §154）");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// 契約 §154.2: 自己除外を `KAMUX_SHIM_DIR` の設定に依存させない。shim
    /// ディレクトリを PATH の**先頭**に置き、`KAMUX_SHIM_DIR` を `env_remove` で
    /// 未設定にしても、自己再帰 exec せず本体（tempdir に置いた偽物）へ届くこと。
    #[test]
    fn the_generated_claude_shim_does_not_self_recurse_when_kamux_shim_dir_is_unset() {
        let base = tempfile::tempdir().expect("tempdir");
        let dir = install_shims(base.path()).expect("install shims");
        let real_dir = base.path().join("real");
        std::fs::create_dir_all(&real_dir).expect("mkdir real");
        let real = real_dir.join("claude");
        std::fs::write(&real, "#!/bin/sh\nprintf 'ARGS:%s\\n' \"$*\"\n").expect("write real");
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        let mut cmd = std::process::Command::new(dir.join("claude"));
        cmd.arg("hello")
            // shim ディレクトリを PATH の先頭に置く（KAMUX_SHIM_DIR 無しでも
            // shim 自身が command -v で見つかる位置）。
            .env("PATH", format!("{}:{}", dir.display(), real_dir.display()))
            .env_remove("KAMUX_SHIM_DIR")
            .env_remove("KAMUX_HOOKS_SETTINGS");

        let out = run_shim_with_deadline(&mut cmd);
        assert!(
            out.status.success(),
            "shim が失敗した: status={:?} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout), "ARGS:hello\n");
    }

    #[test]
    fn the_generated_claude_shim_adds_settings_only_when_the_env_is_set() {
        assert_eq!(run_shim("claude", None), "ARGS:hello\n");
        assert_eq!(
            run_shim("claude", Some("/tmp/kamux-hooks.settings.json")),
            "ARGS:--settings /tmp/kamux-hooks.settings.json hello\n"
        );
    }

    /// 契約 §154.3 ハザード 1 の実測: zsh（macOS の既定シェル）で `PATH` の先頭に
    /// 空要素があり、その要素がカレントディレクトリを意味する場合、名前だけで
    /// 起動されたコマンドの `$0` はディレクトリを含まない（`/` が無い）。POSIX の
    /// `sh`/`bash` はこの位置に `./` を補って `$0` に反映するが、zsh は補わない。
    ///
    /// **この経路は解決していない** —— `case "$0" in */*) … ;; esac` は `/` の
    /// 無い `$0` を誤ってディレクトリ扱いしない（`kamux_self_dir` を空にする）だけで、
    /// `$0` 由来の除外はこの経路で機能しない。
    ///
    /// 実測では実際に自己再帰しなかったが、**`KAMUX_SHIM_DIR` 比較が効いたからでは
    /// ない** —— `KAMUX_SHIM_DIR` を無関係な非空値にしても結果は変わらなかった。
    /// 救っているのは既存の `kamux_path` 構築ロジック（`[ -z "$kamux_path" ]` を
    /// 「まだ何も足していない」の判定に使っている）が、`PATH` の**先頭**にある
    /// 空要素を副作用として必ず落とす性質である（`sh -c 'kamux_path=""; for d in
    /// "" "/a" "/b"; do …; done; echo "$kamux_path"'` で確認済み。`/a:/b` になり
    /// 先頭の空要素は消える。中間・末尾の空要素は落ちない）。**本 PR のスコープ外の
    /// 既存実装（`e640ed9` / Task 21）が持つ性質であり、本 PR が意図して直したもの
    /// ではない。** この副作用が変われば、この経路は再び自己再帰しうる
    /// （契約 §154.3 は「測れ」であって「解け」ではない。§154.6 の「同型の残り」に
    /// 4 行目として報告する）。
    #[test]
    fn dollar_zero_has_no_slash_when_a_leading_empty_path_element_resolves_to_cwd_in_zsh() {
        let base = tempfile::tempdir().expect("tempdir");
        let probe = base.path().join("probe");
        std::fs::write(&probe, "#!/bin/sh\nprintf '%s' \"$0\"\n").expect("write probe");
        std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        let out = std::process::Command::new("zsh")
            .arg("-c")
            .arg("PATH=\":$PATH\" probe")
            .current_dir(base.path())
            .output()
            .expect("run probe via zsh");
        assert!(
            out.status.success(),
            "status={:?} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "probe",
            "zsh の PATH 先頭空要素経由では $0 がコマンド名のまま（/ を含まない）になることを期待した"
        );
    }

    /// 裁定 21-A の実行時の姿: `KAMUX_HOOKS_SETTINGS` が設定されていても
    /// `codex` の shim は引数を変えない。
    #[test]
    fn the_generated_codex_shim_never_changes_the_arguments() {
        assert_eq!(run_shim("codex", None), "ARGS:hello\n");
        assert_eq!(
            run_shim("codex", Some("/tmp/kamux-hooks.settings.json")),
            "ARGS:hello\n"
        );
    }

    /// 契約 §64.3 の逐語（書き換え不可）。
    ///
    /// **`[ -n … ]` のガードを落とす変異はここで赤くなる** —— 素の
    /// `export PATH="$KAMUX_SHIM_DIR:$PATH"` は変数未設定時に `PATH` の先頭へ
    /// 空要素（= カレントディレクトリ）を作る。
    #[test]
    fn the_pty_line_is_the_contract_verbatim() {
        assert_eq!(
            SHELL_PATH_LINE,
            b"[ -n \"$KAMUX_SHIM_DIR\" ] && export PATH=\"$KAMUX_SHIM_DIR:$PATH\"\n"
        );
    }

    /// 契約 §64.3: 「書くのは shim 有効時 かつ `cli_kind == Shell` のときだけ」。
    /// 4 つの `cli_kind` × shim の有効/無効の 8 通りを列挙する。
    #[test]
    fn the_pty_line_is_sent_only_for_shell_with_the_shim_enabled() {
        let dir = PathBuf::from("/tmp/kamux-shim-fixture");
        let every = [
            CliKind::Claude,
            CliKind::Codex,
            CliKind::Shell,
            CliKind::Custom,
        ];
        for cli_kind in every {
            assert_eq!(
                shell_path_line(cli_kind, None),
                None,
                "shim 無効の {cli_kind:?} へ 1 行書いている"
            );
            assert_eq!(
                shell_path_line(cli_kind, Some(&dir)),
                if cli_kind == CliKind::Shell {
                    Some(SHELL_PATH_LINE)
                } else {
                    None
                },
                "cli_kind={cli_kind:?} の判定が契約 §64.3 と違う"
            );
        }
    }
}
