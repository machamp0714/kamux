use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};

pub mod payload;

pub use payload::{parse_hook_event, HookEnvelope, HookEvent, HookKind, WireMessage};

/// macOS の sockaddr_un.sun_path は 104 バイト。
pub const SUN_PATH_MAX: usize = 104;

const RUNTIME_PREFIX: &str = "kamux-hooks-";
const SOCKET_SUFFIX: &str = ".sock";
const SETTINGS_SUFFIX: &str = ".settings.json";

/// 契約 §12.1: $TMPDIR/kamux-hooks-{pid}.sock
pub fn hooks_socket_path() -> AppResult<PathBuf> {
    socket_path_from(&std::env::temp_dir())
}

/// `hooks_socket_path` の本体。`dir` を注入できる形にして、
/// `SUN_PATH_MAX` 境界のオフバイワンをテストで固定できるようにする。
fn socket_path_from(dir: &Path) -> AppResult<PathBuf> {
    let path = dir.join(format!(
        "{RUNTIME_PREFIX}{}{SOCKET_SUFFIX}",
        std::process::id()
    ));
    if path.as_os_str().len() >= SUN_PATH_MAX {
        return Err(AppError::Io(format!(
            "hooks socket path exceeds sun_path limit ({} >= {SUN_PATH_MAX}): {}",
            path.as_os_str().len(),
            path.display()
        )));
    }
    Ok(path)
}

/// --settings に渡す JSON の置き場所。ソケットと同じライフサイクル。
pub fn hooks_settings_path() -> AppResult<PathBuf> {
    Ok(std::env::temp_dir().join(format!(
        "{RUNTIME_PREFIX}{}{SETTINGS_SUFFIX}",
        std::process::id()
    )))
}

/// kamux-hooks-{pid}.sock / kamux-hooks-{pid}.settings.json から pid を取り出す。
pub fn runtime_file_pid(file_name: &str) -> Option<u32> {
    let rest = file_name.strip_prefix(RUNTIME_PREFIX)?;
    let digits = rest
        .strip_suffix(SOCKET_SUFFIX)
        .or_else(|| rest.strip_suffix(SETTINGS_SUFFIX))?;
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u32>().ok()
}

/// アプリが異常終了した回の残骸を掃除する。削除した件数を返す。
/// 設計 §6-6。
pub fn sweep_stale_runtime_files(dir: &Path) -> usize {
    sweep_stale_runtime_files_with(dir, real_kill)
}

/// `sweep_stale_runtime_files` の本体。`socket_path_from` と同じ作法で、
/// テストから固定したい境界（ここでは `kill` の 3 値）を引数へ出す。
/// これが無いと `kill` の第 3 の結果である `EPERM` を通る掃除経路に
/// 観測点を 1 つも置けない（uid に依存せず EPERM を返させる手段が無いため）。
fn sweep_stale_runtime_files_with(
    dir: &Path,
    kill: impl Fn(libc::pid_t, libc::c_int) -> libc::c_int,
) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(pid) = runtime_file_pid(name) else {
            continue;
        };
        if is_pid_alive_with(pid, &kill) {
            continue;
        }
        if std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// `kill(pid, 0)` の戻り値と errno から生存判定を行う純関数。
/// `rc == 0` は生存。`rc != 0` のとき `EPERM`（他人の pid で権限が無く存在確認できない
/// だけで、プロセス自体は存在する）は「生きている」とみなす。`ESRCH`（プロセス不在）は
/// 「死んでいる」とみなす。
///
/// uid に依存する `kill(1, 0)` の実際の呼び出し結果ではなく、この関数自体を
/// `0` / `EPERM` / `ESRCH` の 3 値で固定してテストする。root で実行すると
/// `kill(1, 0)` は `rc == 0` を返して EPERM 分岐を通らないため、実呼び出し経由の
/// テストは uid に依存してしまう。
fn classify_kill_result(rc: libc::c_int, errno: Option<i32>) -> bool {
    if rc == 0 {
        return true;
    }
    errno == Some(libc::EPERM)
}

/// `kill(pid, 0)` の実呼び出し。注入の継ぎ目をこの 1 行に閉じ込める。
/// ここには分岐も値の加工も無いので、テストできない部分は「判断の無い受け渡し」だけになる。
fn real_kill(pid: libc::pid_t, sig: libc::c_int) -> libc::c_int {
    // SAFETY: シグナル 0 の kill は副作用がなく、存在確認のみを行う。
    unsafe { libc::kill(pid, sig) }
}

/// kill(pid, 0) で存在確認する。権限エラー(EPERM)は「生きている」とみなす。
///
/// 注入するのは **`kill` そのもの**であり、`(rc, errno)` の組を返す関数ではない。
/// 組を返す形にすると errno を読む判断が注入側へ移動し、守りたい配線がテスト
/// 対象の外へ出てしまう。errno の読み取りをここに残すことで、`rc != 0` の
/// ときに errno が `classify_kill_result` まで実際に届くことを観測できる
/// （`rc == 0` のとき errno を読まない側の分岐は、`classify_kill_result` が
/// `rc` を先に見るため、この配線からは観測できない。下の分岐のコメント参照）。
fn is_pid_alive_with(pid: u32, kill: impl Fn(libc::pid_t, libc::c_int) -> libc::c_int) -> bool {
    let rc = kill(pid as libc::pid_t, 0);
    // 防御的分岐: `classify_kill_result` は `rc == 0` を errno より先に見るため、
    // ここで errno を読まないことは同関数越しには観測できない
    // (`classify(0, Some(_))` も `classify(0, None)` も `true`)。それでも
    // 直前の syscall の errno を生存判定に混入させない意図で残している。
    let errno = if rc == 0 {
        None
    } else {
        std::io::Error::last_os_error().raw_os_error()
    };
    classify_kill_result(rc, errno)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_is_in_tmpdir_and_contains_pid() {
        let p = hooks_socket_path().expect("socket path");
        assert!(p.starts_with(std::env::temp_dir()));
        let name = p.file_name().expect("name").to_string_lossy().into_owned();
        assert_eq!(name, format!("kamux-hooks-{}.sock", std::process::id()));
        assert!(
            p.as_os_str().len() < SUN_PATH_MAX,
            "path too long for sun_path: {}",
            p.display()
        );
    }

    #[test]
    fn settings_path_sits_next_to_the_socket() {
        let s = hooks_settings_path().expect("settings path");
        let name = s.file_name().expect("name").to_string_lossy().into_owned();
        assert_eq!(
            name,
            format!("kamux-hooks-{}.settings.json", std::process::id())
        );
    }

    #[test]
    fn extracts_pid_from_runtime_file_names() {
        assert_eq!(runtime_file_pid("kamux-hooks-4321.sock"), Some(4321));
        assert_eq!(
            runtime_file_pid("kamux-hooks-4321.settings.json"),
            Some(4321)
        );
        assert_eq!(runtime_file_pid("kamux-hooks-.sock"), None);
        assert_eq!(runtime_file_pid("kamux-hooks-abc.sock"), None);
        assert_eq!(runtime_file_pid("unrelated.sock"), None);
        assert_eq!(runtime_file_pid("kamux-relay-test-1.sock"), None);
    }

    /// `socket_path_from` が組み立てる相対部分（ファイル名）の長さ。
    /// 境界テストで dir の長さを逆算するために使う。
    fn file_name_len() -> usize {
        format!("kamux-hooks-{}.sock", std::process::id()).len()
    }

    /// `total_len` バイトちょうどのソケットパスになる `dir` を組み立てる。
    /// `dir.join(file_name)` は dir がセパレータで終わらない限り "/" を 1 つ挟むため、
    /// `dir` の長さは `total_len - 1(sep) - file_name_len` にする。
    fn dir_for_total_path_len(total_len: usize) -> PathBuf {
        let dir_len = total_len - 1 - file_name_len();
        // 先頭の "/" を含めて dir_len バイトにする。
        let dir = PathBuf::from(format!("/{}", "a".repeat(dir_len - 1)));
        // 逆算が正しいことをここで固定する。ずれたら境界テストの Ok/Err の判定を
        // 待たずに、この assert 自体が落ちる（103/104 いずれの呼び出しでも赤になる）。
        assert_eq!(
            dir.join(format!("kamux-hooks-{}.sock", std::process::id()))
                .as_os_str()
                .len(),
            total_len
        );
        dir
    }

    #[test]
    fn socket_path_from_accepts_a_path_of_exactly_103_bytes() {
        let dir = dir_for_total_path_len(SUN_PATH_MAX - 1);
        let path = socket_path_from(&dir).expect("103 bytes must be accepted");
        assert_eq!(path.as_os_str().len(), SUN_PATH_MAX - 1);
    }

    #[test]
    fn socket_path_from_rejects_a_path_of_exactly_104_bytes() {
        let dir = dir_for_total_path_len(SUN_PATH_MAX);
        let err = socket_path_from(&dir);
        assert!(err.is_err(), "104 bytes must be rejected");
    }

    #[test]
    fn classify_kill_result_treats_rc_zero_as_alive() {
        assert!(classify_kill_result(0, None));
    }

    #[test]
    fn classify_kill_result_treats_eperm_as_alive() {
        assert!(classify_kill_result(-1, Some(libc::EPERM)));
    }

    #[test]
    fn classify_kill_result_treats_esrch_as_dead() {
        assert!(!classify_kill_result(-1, Some(libc::ESRCH)));
    }

    /// `kill` を `rc` と errno の組で偽装する。uid に一切依存しない。
    ///
    /// `libc::__error()` は macOS の errno 変数へのポインタ（契約 §0: 対象 OS は
    /// macOS のみ。`cfg` 分岐は導入しない）。同ファイルは既に `SUN_PATH_MAX = 104`
    /// と生の `libc::kill` で macOS を前提にしている。
    fn fake_kill(
        rc: libc::c_int,
        errno: libc::c_int,
    ) -> impl Fn(libc::pid_t, libc::c_int) -> libc::c_int {
        move |_pid, _sig| {
            // SAFETY: __error() はこのスレッドの errno を指す。書き込みはスレッドローカル。
            unsafe { *libc::__error() = errno };
            rc
        }
    }

    /// 配線の守り: `kill` が `EPERM` を返したとき、errno が
    /// `classify_kill_result` まで実際に届くこと。
    /// errno の取得を潰す変異（両分岐とも `None`）でここが赤になる。
    #[test]
    fn is_pid_alive_with_reads_eperm_from_errno() {
        assert!(is_pid_alive_with(4_194_303, fake_kill(-1, libc::EPERM)));
    }

    /// 同じ配線の裏側。`ESRCH` は「死んでいる」として届くこと。
    #[test]
    fn is_pid_alive_with_reads_esrch_from_errno() {
        assert!(!is_pid_alive_with(4_194_303, fake_kill(-1, libc::ESRCH)));
    }

    /// `kill` が `rc == 0` を返したときは、errno の値に関わらず「生存」と
    /// 判定すること（`classify_kill_result` が `rc` を errno より先に見る
    /// ため）。ここでは直前の syscall が残しうる `ESRCH` を errno に仕込んで
    /// も結果が変わらないことを確認する。
    ///
    /// 注意: この主張は「`rc == 0` のとき errno を読まない」ことまでは
    /// 検証していない。`is_pid_alive_with` 内の `if rc == 0 { None }` 分岐は
    /// `classify_kill_result` 越しには観測できないため（`classify(0, Some(_))`
    /// も `classify(0, None)` も `true`)。
    #[test]
    fn is_pid_alive_with_reports_alive_when_kill_succeeds_regardless_of_errno() {
        assert!(is_pid_alive_with(4_194_303, fake_kill(0, libc::ESRCH)));
    }

    /// `sweeps_only_dead_pids` のフィクスチャには `kill` の第 3 の結果である
    /// `EPERM` が無い（自 pid の `rc == 0` と存在しない pid の `ESRCH` だけ）。
    /// EPERM を返す pid のファイルが掃除経路で残ることをここで固定する。
    #[test]
    fn sweep_keeps_files_whose_pid_reports_eperm() {
        let dir = std::env::temp_dir().join(format!(
            "kamux-sweep-eperm-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");

        let eperm_sock = dir.join("kamux-hooks-4194303.sock");
        std::fs::write(&eperm_sock, b"").expect("write");

        let removed = sweep_stale_runtime_files_with(&dir, fake_kill(-1, libc::EPERM));

        assert_eq!(removed, 0, "EPERM は生存扱いなので消してはならない");
        assert!(eperm_sock.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sweeps_only_dead_pids() {
        let dir = std::env::temp_dir().join(format!("kamux-sweep-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");

        // 生きている pid（自分自身）→ 残す
        let alive = dir.join(format!("kamux-hooks-{}.sock", std::process::id()));
        std::fs::write(&alive, b"").expect("write");

        // 存在しない pid → 消す。pid 上限を超えた値を使う
        let dead_sock = dir.join("kamux-hooks-4194303.sock");
        let dead_settings = dir.join("kamux-hooks-4194303.settings.json");
        std::fs::write(&dead_sock, b"").expect("write");
        std::fs::write(&dead_settings, b"").expect("write");

        // 無関係なファイル → 残す
        let unrelated = dir.join("something-else.sock");
        std::fs::write(&unrelated, b"").expect("write");

        let removed = sweep_stale_runtime_files(&dir);

        assert_eq!(removed, 2);
        assert!(alive.exists());
        assert!(!dead_sock.exists());
        assert!(!dead_settings.exists());
        assert!(unrelated.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
