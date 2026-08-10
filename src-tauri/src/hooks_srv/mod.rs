use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::error::{AppError, AppResult};

pub mod handler;
pub mod payload;

pub use handler::HookHandler;
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

/// 1 メッセージの受け入れ上限。Stop の last_assistant_message は長くなりうる。
const MAX_MESSAGE_BYTES: u64 = 1024 * 1024;
/// 悪意ある/切れた接続が accept ループを止められる時間の上限。
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// hooks_srv → 状態機械の唯一の依存。本番実装は HookHandler（Task 12）。
pub trait HookSink: Send + Sync + 'static {
    fn on_hook(&self, event: HookEvent);
}

/// Unix ソケットの accept ループを持つ。Drop で停止とファイル削除を行う。
pub struct HooksServer {
    socket_path: PathBuf,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl HooksServer {
    pub fn start(socket_path: PathBuf, sink: Arc<dyn HookSink>) -> AppResult<Self> {
        // 前回の残骸（同 pid の再利用など）があれば消してから bind する。
        let _ = std::fs::remove_file(&socket_path);

        let listener = UnixListener::bind(&socket_path).map_err(|e| {
            AppError::Io(format!(
                "failed to bind hooks socket {}: {e}",
                socket_path.display()
            ))
        })?;

        // umask 依存にせず明示的に 0600 にする（設計 §6-6）。
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| AppError::Io(format!("failed to chmod hooks socket: {e}")))?;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);

        let handle = std::thread::Builder::new()
            .name("kamux-hooks-srv".into())
            .spawn(move || accept_loop(listener, sink, stop_for_thread))
            .map_err(|e| AppError::Io(format!("failed to spawn hooks thread: {e}")))?;

        Ok(Self {
            socket_path,
            stop,
            handle: Some(handle),
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// 冪等。二重に呼んでも安全。
    pub fn shutdown(&mut self) {
        let Some(handle) = self.handle.take() else {
            return;
        };
        self.stop.store(true, Ordering::SeqCst);
        // ブロッキング accept を起こすためだけの接続。
        let _ = UnixStream::connect(&self.socket_path);
        let _ = handle.join();
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

impl Drop for HooksServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// `accept()` が持続的に失敗するとき（例: プロセスの fd 上限 `EMFILE`）に待つ時間。
/// 連続失敗回数に応じて線形に増え、上限に張り付く。
///
/// 契約 §0（アイドル CPU ほぼ 0%・ポーリングループ禁止）向けの手当て。
/// `std::os::unix::net::Incoming::next()` はエラーでも `None` を返さず
/// `Some(listener.accept().map(...))` を返し続けるため、この待機を入れないと
/// `accept()` が持続的に失敗する状態で `accept_loop` は 1 ミリ秒も待たずに
/// 回り続け、CPU を 1 コア占有しながら `warn!` を出し続ける。
const ACCEPT_ERROR_BACKOFF_UNIT: Duration = Duration::from_millis(50);
const ACCEPT_ERROR_BACKOFF_MAX: Duration = Duration::from_secs(1);

fn accept_error_backoff(consecutive_errors: u32) -> Duration {
    ACCEPT_ERROR_BACKOFF_UNIT
        .saturating_mul(consecutive_errors)
        .min(ACCEPT_ERROR_BACKOFF_MAX)
}

/// ブロッキング accept。正常系の待機中は CPU を消費しない（契約 §0）。
/// 受信処理をインラインで行うことでイベント順序を保存する（設計 §6-4）。
fn accept_loop(listener: UnixListener, sink: Arc<dyn HookSink>, stop: Arc<AtomicBool>) {
    accept_loop_with(listener.incoming(), sink, stop, std::thread::sleep);
}

/// `accept_loop` の本体。`socket_path_from` / `sweep_stale_runtime_files_with` と
/// 同じ作法で、テストから固定したい境界（`incoming` の連続失敗と、それに対する
/// 待機の実行）を引数へ出す。実 `UnixListener` を `accept()` が持続的に失敗する
/// 状態にするのは困難なので、この継ぎ目が無いと「無待機スピンを防ぐ手当てが
/// 実際に呼ばれていること」を検証できない。
fn accept_loop_with<I>(
    incoming: I,
    sink: Arc<dyn HookSink>,
    stop: Arc<AtomicBool>,
    sleep: impl Fn(Duration),
) where
    I: Iterator<Item = std::io::Result<UnixStream>>,
{
    let mut consecutive_errors: u32 = 0;
    for stream in incoming {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        match stream {
            Ok(stream) => {
                consecutive_errors = 0;
                handle_connection(stream, sink.as_ref());
            }
            Err(e) => {
                consecutive_errors = consecutive_errors.saturating_add(1);
                tracing::warn!(error = %e, "hooks socket accept failed");
                // ポーリングではない: 正常系はブロッキング accept のままで、
                // ここはエラーが続く異常系でのみ通るスピン抑止。
                sleep(accept_error_backoff(consecutive_errors));
            }
        }
    }
}

fn handle_connection(stream: UnixStream, sink: &dyn HookSink) {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));

    let mut buf = Vec::new();
    // 読み取りが途中でタイムアウトしても、取れた分だけパースを試みる。
    let _ = stream.take(MAX_MESSAGE_BYTES).read_to_end(&mut buf);
    if buf.is_empty() {
        return;
    }
    // read_to_end は上限に当たっても Ok を返す。切り詰めは後段の parse 失敗として
    // 現れるだけなので、原因が分かる警告をここで出す。
    if buf.len() as u64 == MAX_MESSAGE_BYTES {
        tracing::warn!(
            limit = MAX_MESSAGE_BYTES,
            "hook message hit the size cap and was likely truncated"
        );
    }

    match parse_hook_event(&buf) {
        Ok(event) => sink.on_hook(event),
        Err(e) => {
            // 握りつぶさず生 JSON を残す（契約 §12.5）。
            tracing::warn!(
                error = %e,
                raw = %String::from_utf8_lossy(&buf),
                "failed to parse hook wire message"
            );
        }
    }
}

/// hooks が有効なとき AppState に載る。relay 解決に失敗したら None になり、
/// セッションは hooks 無しで動く(設計書 §12「hooks 不達 → 汎用ヒューリスティック」)。
#[derive(Debug, Clone)]
pub struct HooksRuntime {
    pub socket_path: PathBuf,
    pub settings_path: PathBuf,
    pub relay_bin: PathBuf,
}

/// 同梱される relay バイナリのファイル名。
/// Tauri の externalBin はターゲットトリプルのサフィックスを剥がして配置する。
pub const RELAY_BIN_NAME: &str = "kamux-relay";

/// テスト可能な純粋版。
///
/// `KAMUX_RELAY_BIN`（開発・テスト用の明示指定）が与えられた場合は、それだけを
/// 見て fail-fast する。ファイルが無ければ即座にエラーを返し、2 のディレクトリ
/// 探索へはフォールスルーしない（古い relay が居座っていても override が優先
/// されることを保証するため）。
///
/// 1. `KAMUX_RELAY_BIN`（開発・テスト用の明示指定。指すファイルが無ければ即エラー）
/// 2. 上記が無い場合、アプリ実行ファイルと同じディレクトリ
///    - dev: `target/debug/kamux` の隣の `target/debug/kamux-relay`
///    - 本番: `kamux.app/Contents/MacOS/` （設計 §6-1 / 未確認事実 #13。Task 16 で実測）
pub fn resolve_relay_bin_from(env_override: Option<&str>, exe_dir: &Path) -> AppResult<PathBuf> {
    if let Some(raw) = env_override {
        let path = PathBuf::from(raw);
        if path.is_file() {
            return Ok(path);
        }
        return Err(AppError::CliNotFound(format!(
            "KAMUX_RELAY_BIN does not point to a file: {}",
            path.display()
        )));
    }

    let beside = exe_dir.join(RELAY_BIN_NAME);
    if beside.is_file() {
        return Ok(beside);
    }

    Err(AppError::CliNotFound(format!(
        "{RELAY_BIN_NAME} not found in {}",
        exe_dir.display()
    )))
}

/// 実環境版。
pub fn resolve_relay_bin() -> AppResult<PathBuf> {
    let env_override = std::env::var("KAMUX_RELAY_BIN").ok();
    let exe =
        std::env::current_exe().map_err(|e| AppError::Io(format!("current_exe failed: {e}")))?;
    let exe_dir = exe
        .parent()
        .ok_or_else(|| AppError::Io(format!("current_exe has no parent: {}", exe.display())))?
        .to_path_buf();
    resolve_relay_bin_from(env_override.as_deref(), &exe_dir)
}

/// テスト可能な内部版。パスと relay 解決結果を注入する。
pub fn bootstrap_hooks_in(
    sink: Arc<dyn HookSink>,
    relay_bin: AppResult<PathBuf>,
    socket_path: PathBuf,
    settings_path: PathBuf,
) -> (Option<HooksRuntime>, Option<HooksServer>) {
    // 設計書 §12: hooks が使えなくてもアプリは起動し、汎用ヒューリスティックへ落ちる。
    let relay_bin = match relay_bin {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "hooks disabled: kamux-relay not found");
            return (None, None);
        }
    };

    let server = match HooksServer::start(socket_path.clone(), sink) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "hooks disabled: failed to start hooks socket");
            return (None, None);
        }
    };

    if let Err(e) = crate::session::cli_args::write_hook_settings_file(&settings_path, &relay_bin) {
        tracing::warn!(error = %e, "hooks disabled: failed to write settings file");
        return (None, None);
    }

    let runtime = HooksRuntime {
        socket_path,
        settings_path,
        relay_bin,
    };
    tracing::info!(socket = %runtime.socket_path.display(), "hooks enabled");
    (Some(runtime), Some(server))
}

/// 実環境版。アプリ起動時に 1 回だけ呼ぶ。
pub fn bootstrap_hooks(sink: Arc<dyn HookSink>) -> (Option<HooksRuntime>, Option<HooksServer>) {
    // 前回の異常終了で残ったソケット/設定を掃除する（設計 §6-6）。
    let removed = sweep_stale_runtime_files(&std::env::temp_dir());
    if removed > 0 {
        tracing::info!(removed, "swept stale hooks runtime files");
    }

    let socket_path = match hooks_socket_path() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "hooks disabled");
            return (None, None);
        }
    };
    let settings_path = match hooks_settings_path() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "hooks disabled");
            return (None, None);
        }
    };

    bootstrap_hooks_in(sink, resolve_relay_bin(), socket_path, settings_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<HookEvent>>,
    }

    impl RecordingSink {
        fn snapshot(&self) -> Vec<HookEvent> {
            self.events.lock().expect("lock").clone()
        }
    }

    impl HookSink for RecordingSink {
        fn on_hook(&self, event: HookEvent) {
            self.events.lock().expect("lock").push(event);
        }
    }

    fn test_socket_path(tag: &str) -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("kamux-srvtest-{}-{}.sock", tag, std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn send(path: &Path, message: &str) {
        let mut s = UnixStream::connect(path).expect("connect");
        s.write_all(message.as_bytes()).expect("write");
        s.flush().expect("flush");
        s.shutdown(std::net::Shutdown::Write).expect("shutdown");
    }

    fn wait_for<F: Fn() -> bool>(cond: F) {
        for _ in 0..200 {
            if cond() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("condition not met within 2s");
    }

    /// `wait_for` と同じ形だが、上限を明示的に指定できる。
    ///
    /// `wait_for` の固定上限（200 * 10ms = 2 秒）は `READ_TIMEOUT`（2 秒）と
    /// ちょうど同じ長さである。黙り込む接続の read タイムアウトを跨いで待つ
    /// テストで `wait_for` をそのまま使うと、「read タイムアウト待ち」と
    /// 「テスト自身のタイムアウト」が競走してしまい、CI 環境の遅さ次第で
    /// 揺れる。そのテスト専用に、`READ_TIMEOUT` より十分長い独立の上限を渡す。
    fn wait_for_within<F: Fn() -> bool>(bound: Duration, cond: F) {
        let start = std::time::Instant::now();
        loop {
            if cond() {
                return;
            }
            assert!(
                start.elapsed() < bound,
                "condition not met within {bound:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn creates_socket_with_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let path = test_socket_path("perm");
        let sink = Arc::new(RecordingSink::default());
        let server = HooksServer::start(path.clone(), sink).expect("start");

        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "socket must be owner-only, got {mode:o}");

        drop(server);
    }

    #[test]
    fn delivers_events_in_arrival_order() {
        let path = test_socket_path("order");
        let sink = Arc::new(RecordingSink::default());
        let server = HooksServer::start(path.clone(), sink.clone()).expect("start");

        for kind in ["SessionStart", "Notification", "Stop"] {
            send(
                &path,
                &format!(
                    r#"{{"v":1,"kamux_session_id":"3f2a0000-0000-4000-8000-000000009c1e","hook_kind":"{kind}","payload":{{"session_id":"550e8400"}}}}"#
                ),
            );
        }

        wait_for(|| sink.snapshot().len() == 3);
        let events = sink.snapshot();
        assert_eq!(events[0].kind, HookKind::SessionStart);
        assert_eq!(events[1].kind, HookKind::Notification);
        assert_eq!(events[2].kind, HookKind::Stop);
        assert_eq!(events[2].claude_session_id.as_deref(), Some("550e8400"));
        // §81.3: kamux_session_id と claude_session_id は同じ String 型で名前が
        // 同義に読める。上のアサートは claude_session_id しか見ておらず、2 つの
        // フィールドが取り違えられても検出できない。ここで kamux_session_id も
        // 固定し、取り違えたら別物になる具体値（wire の "3f2a0000-..." と
        // payload の "550e8400"）で区別する。
        assert_eq!(
            events[2].kamux_session_id,
            "3f2a0000-0000-4000-8000-000000009c1e"
        );

        drop(server);
    }

    #[test]
    fn broken_message_is_dropped_and_the_loop_survives() {
        let path = test_socket_path("broken");
        let sink = Arc::new(RecordingSink::default());
        let server = HooksServer::start(path.clone(), sink.clone()).expect("start");

        send(&path, "this is not json");
        send(
            &path,
            r#"{"v":1,"kamux_session_id":"3f2a0000-0000-4000-8000-000000009c1e","hook_kind":"Stop","payload":null}"#,
        );

        wait_for(|| sink.snapshot().len() == 1);
        assert_eq!(sink.snapshot()[0].kind, HookKind::Stop);

        drop(server);
    }

    /// 契約 §0 の並行処理要件（brief §3）: 「1 つの接続の相手が黙り込んでも、
    /// サーバ全体が止まらないこと」。accept ループは受信処理をインラインで
    /// 行う（設計 §6-4）ため、黙り込む接続を先に受け取ると、後続の接続は
    /// その接続の read が `READ_TIMEOUT`（2 秒）で打ち切られるまで accept
    /// されない。「止まらない」とは「無期限には止まらない」ことであり、
    /// このテストは READ_TIMEOUT を跨いで後続メッセージが届くことを固定する。
    #[test]
    fn a_silent_peer_does_not_block_delivery_of_a_later_message() {
        let path = test_socket_path("silent-peer");
        let sink = Arc::new(RecordingSink::default());
        let server = HooksServer::start(path.clone(), sink.clone()).expect("start");

        // 黙り込む接続: 何も書かず、閉じずに繋ぎっぱなしにする。
        let silent = UnixStream::connect(&path).expect("connect silent peer");

        send(
            &path,
            r#"{"v":1,"kamux_session_id":"3f2a0000-0000-4000-8000-000000009c1e","hook_kind":"Stop","payload":null}"#,
        );

        // READ_TIMEOUT（2 秒）を跨ぐ必要があるので、wait_for の固定 2 秒とは
        // 競走しない、十分に長い独立の上限を使う。
        wait_for_within(Duration::from_secs(6), || sink.snapshot().len() == 1);
        assert_eq!(sink.snapshot()[0].kind, HookKind::Stop);

        drop(silent);
        drop(server);
    }

    #[test]
    fn shutdown_unlinks_the_socket() {
        let path = test_socket_path("unlink");
        let sink = Arc::new(RecordingSink::default());
        let mut server = HooksServer::start(path.clone(), sink).expect("start");
        assert!(path.exists());

        server.shutdown();

        assert!(!path.exists(), "socket file must be removed on shutdown");
        // 二重 shutdown が panic しないこと（Drop でもう一度呼ばれる）
        server.shutdown();
    }

    #[test]
    fn start_replaces_a_stale_socket_file() {
        let path = test_socket_path("stale");
        std::fs::write(&path, b"leftover").expect("write stale file");

        let sink = Arc::new(RecordingSink::default());
        let server = HooksServer::start(path.clone(), sink.clone()).expect("start over stale file");

        send(
            &path,
            r#"{"v":1,"kamux_session_id":"3f2a0000-0000-4000-8000-000000009c1e","hook_kind":"Notification","payload":null}"#,
        );
        wait_for(|| sink.snapshot().len() == 1);

        drop(server);
    }

    /// PR 15 からの持ち越し 1（brief 冒頭）: relay の実出力を `parse_hook_event` が
    /// 受ける E2E テストを 1 本置く。手書き JSON リテラルだけでは、relay 側の
    /// wire フォーマット変更に誰も気づけない。
    ///
    /// 実 `kamux-relay` バイナリを起動し、素の `UnixListener` でバイト列を採取
    /// してから `parse_hook_event` へ渡す（サーバの accept ループを経由しない
    /// ことで、「relay の出力」と「hooks_srv の読み取り」を別々に固定できる）。
    fn e2e_relay_bin_path() -> PathBuf {
        let exe = std::env::current_exe().expect("current_exe for the test binary itself");
        let profile_dir = exe
            .parent() // .../target/debug/deps/
            .and_then(Path::parent) // .../target/debug/
            .expect("test binary must live under target/<profile>/deps/");
        let candidate = profile_dir.join("kamux-relay");
        assert!(
            candidate.exists(),
            "kamux-relay binary not found at {} (run `cargo build --workspace` first)",
            candidate.display()
        );
        candidate
    }

    /// 実 relay プロセスを起動し、素のソケットで 1 メッセージ分のバイト列を採取する。
    /// accept と read の両方を 1 つの上限（10 秒）の内側に収める
    /// （契約の並行処理要件・§3 と同じ形。relay が繋がらない/送らないケースを
    /// 無期限ハングさせない）。
    fn capture_one_relay_message(sock: &Path, hook_kind: &str, stdin: &[u8]) -> Vec<u8> {
        use std::io::Read;
        use std::sync::mpsc;

        let listener = UnixListener::bind(sock).expect("bind capture listener");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| -> std::io::Result<Vec<u8>> {
                let (mut stream, _) = listener.accept()?;
                let mut buf = Vec::new();
                stream.read_to_end(&mut buf)?;
                Ok(buf)
            })();
            let _ = tx.send(result);
        });

        let relay = e2e_relay_bin_path();
        let sock_for_child = sock.to_path_buf();
        let stdin_owned = stdin.to_vec();
        let hook_kind_owned = hook_kind.to_string();
        let child = std::thread::spawn(move || {
            let mut proc = std::process::Command::new(relay)
                .arg(hook_kind_owned)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .env("KAMUX_SESSION_ID", "3f2a0000-0000-4000-8000-000000009c1e")
                .env("KAMUX_HOOKS_SOCK", &sock_for_child)
                .spawn()
                .expect("spawn relay");
            let _ = proc.stdin.as_mut().expect("stdin").write_all(&stdin_owned);
            proc.wait_with_output().expect("wait")
        });

        let buf = match rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(Ok(buf)) => buf,
            Ok(Err(e)) => panic!("listener.accept() または read_to_end が失敗した: {e}"),
            Err(_) => panic!(
                "relay が 10 秒以内にソケットへ接続しなかった \
                 (relay が接続していないか、接続後に何も送っていない可能性がある)"
            ),
        };

        let out = child.join().expect("join relay process");
        assert_eq!(out.status.code(), Some(0), "relay must always exit 0");
        assert!(out.stdout.is_empty());
        assert!(out.stderr.is_empty());

        let _ = std::fs::remove_file(sock);
        buf
    }

    /// `real_relay_output_is_understood_by_parse_hook_event_for_each_registered_hook_kind`
    /// の 1 ケース分。フィールドが多いタプルは clippy::type_complexity に触れるため
    /// 名前付き構造体にする。
    struct RegisteredHookCase {
        hook_kind: &'static str,
        stdin: &'static [u8],
        expected_kind: HookKind,
        expected_claude_session_id: Option<&'static str>,
        expected_source: Option<&'static str>,
    }

    /// kamux が登録する 4 種の hook（契約 §12.4）それぞれについて、実 relay が
    /// 生成したバイト列が `parse_hook_event` で正しく読めることを固定する。
    /// PR 単位レビュー（brief 冒頭・持ち越し 1）が手作業で確認した内容の恒久化。
    #[test]
    fn real_relay_output_is_understood_by_parse_hook_event_for_each_registered_hook_kind() {
        let cases = [
            RegisteredHookCase {
                hook_kind: "SessionStart",
                stdin: br#"{"session_id":"550e8400-e29b-41d4-a716-446655440000","hook_event_name":"SessionStart","source":"startup"}"#,
                expected_kind: HookKind::SessionStart,
                expected_claude_session_id: Some("550e8400-e29b-41d4-a716-446655440000"),
                expected_source: Some("startup"),
            },
            RegisteredHookCase {
                hook_kind: "Notification",
                stdin: br#"{"totally":"unknown","shape":[1,2,3]}"#,
                expected_kind: HookKind::Notification,
                expected_claude_session_id: None,
                expected_source: None,
            },
            RegisteredHookCase {
                hook_kind: "PermissionRequest",
                stdin: br#"{"shape":"entirely unknown"}"#,
                expected_kind: HookKind::PermissionRequest,
                expected_claude_session_id: None,
                expected_source: None,
            },
            RegisteredHookCase {
                hook_kind: "Stop",
                stdin: br#"{"session_id":"abc123","hook_event_name":"Stop","stop_hook_active":false}"#,
                expected_kind: HookKind::Stop,
                expected_claude_session_id: Some("abc123"),
                expected_source: None,
            },
        ];

        for case in cases {
            let hook_kind = case.hook_kind;
            let sock = test_socket_path(&format!("e2e-{hook_kind}"));
            let buf = capture_one_relay_message(&sock, hook_kind, case.stdin);

            // 契約 §84.3: 6 フィールドすべてが名前・省略可否まで一致すること。
            // 成功系（payload が解釈できた）では raw_base64 / truncated は現れない。
            let value: serde_json::Value = serde_json::from_slice(&buf).expect("wire is JSON");
            assert_eq!(value["v"], 1);
            assert_eq!(
                value["kamux_session_id"],
                "3f2a0000-0000-4000-8000-000000009c1e"
            );
            assert_eq!(value["hook_kind"], hook_kind);
            assert!(value["payload"].is_object());
            assert!(
                value.get("raw_base64").is_none(),
                "raw_base64 must be absent when payload parses: {value}"
            );
            assert!(
                value.get("truncated").is_none(),
                "truncated must be absent for a non-truncated message: {value}"
            );

            let event = parse_hook_event(&buf).expect("real relay bytes must parse");
            assert_eq!(event.kind, case.expected_kind, "hook_kind={hook_kind}");
            assert_eq!(
                event.kamux_session_id, "3f2a0000-0000-4000-8000-000000009c1e",
                "hook_kind={hook_kind}"
            );
            assert_eq!(
                event.claude_session_id.as_deref(),
                case.expected_claude_session_id,
                "hook_kind={hook_kind}"
            );
            assert_eq!(
                event.source.as_deref(),
                case.expected_source,
                "hook_kind={hook_kind}"
            );
        }
    }

    /// 契約 §84.3: relay が stdin の読み取り上限（1 MiB）に当たったとき、
    /// `raw_base64` は先頭 4 KiB（5464 base64 文字）に切り詰められ、`payload` は
    /// null になる。それでも `hook_kind` は argv 由来なので `parse_hook_event` は
    /// 正しい種別を返す（PR 単位レビュー・持ち越し 1 の確認の恒久化）。
    #[test]
    fn real_relay_truncated_output_flows_through_parse_hook_event_with_capped_raw_base64() {
        const OVER_MAX_STDIN_BYTES: usize = 1024 * 1024 + 1;
        let oversized = vec![b'x'; OVER_MAX_STDIN_BYTES];

        let sock = test_socket_path("e2e-truncated");
        let buf = capture_one_relay_message(&sock, "Stop", &oversized);

        let value: serde_json::Value = serde_json::from_slice(&buf).expect("wire is JSON");
        assert_eq!(value["truncated"], true);
        assert!(value["payload"].is_null());
        let raw_base64 = value["raw_base64"].as_str().expect("raw_base64 present");
        assert_eq!(
            raw_base64.len(),
            5464,
            "raw_base64 must be capped to exactly 4 KiB (5464 base64 chars), got {}",
            raw_base64.len()
        );

        let event = parse_hook_event(&buf).expect("truncated bytes must still parse");
        assert_eq!(event.kind, HookKind::Stop);
        assert_eq!(
            event.kamux_session_id,
            "3f2a0000-0000-4000-8000-000000009c1e"
        );
        assert_eq!(event.claude_session_id, None);
    }

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

    /// 契約 §0: `accept()` が持続的に失敗する状況（`EMFILE` 等）で無待機スピンに
    /// ならないこと。実 `UnixListener` を持続的に失敗させるのは困難なので、
    /// `accept_loop_with` の `incoming` に合成のエラー列を注入する
    /// （`sweep_stale_runtime_files_with` と同じ作法）。`sleep` も注入し、
    /// 実時間を消費せずに「何回・どれだけ待とうとしたか」を記録する。
    ///
    /// 記録した待機列を、1 回目から 30 回目までの期待値（線形増加し `MAX` で
    /// 頭打ちになる列）と `assert_eq!` で突き合わせる。`> Duration::ZERO` のような
    /// 恒真に近い述語では「線形に増える」ことも「上限に張り付く」ことも守れない
    /// ため、この 1 本で以下をまとめて守る:
    /// - 待機の呼び出しそのものを消す/定数に潰す変異 → 列が一致せず赤
    /// - 失敗回数の上限（cap）を外す変異 → 30 回目には UNIT*30 (=1.5s) が
    ///   期待値 `MAX` (=1s) と一致せず赤
    #[test]
    fn accept_loop_backs_off_and_caps_wait_when_accept_fails_persistently() {
        const PERSISTENT_FAILURES: usize = 30;
        // このテストの「cap（上限への張り付き）」カバレッジは、doc コメント
        // （直上）がハードコードしている `UNIT*30 (=1.5s) > MAX (=1s)` の関係が
        // 実際に成立していることに依存する。定数のどちらかを動かしてこの関係が
        // 崩れると、下の expected は cap 分岐を一度も通らずに線形加算のみへ
        // 縮退し、cap のカバレッジが黙って消える。doc に書くだけでなく、ここで
        // assert して壊れたら赤にする（契約 §90.2）。
        assert!(
            ACCEPT_ERROR_BACKOFF_UNIT.saturating_mul(PERSISTENT_FAILURES as u32)
                > ACCEPT_ERROR_BACKOFF_MAX,
            "fixture invariant broken: UNIT * PERSISTENT_FAILURES must exceed MAX so the \
             cap branch is actually exercised below"
        );
        let errors = (0..PERSISTENT_FAILURES).map(|_| -> std::io::Result<UnixStream> {
            Err(std::io::Error::from_raw_os_error(libc::EMFILE))
        });

        let sink: Arc<dyn HookSink> = Arc::new(RecordingSink::default());
        let waits: Arc<Mutex<Vec<Duration>>> = Arc::new(Mutex::new(Vec::new()));
        let waits_for_sleep = Arc::clone(&waits);

        accept_loop_with(errors, sink, Arc::new(AtomicBool::new(false)), move |d| {
            waits_for_sleep.lock().expect("lock").push(d)
        });

        let waits = waits.lock().expect("lock").clone();
        assert_eq!(
            waits.len(),
            PERSISTENT_FAILURES,
            "each persistent accept failure must wait exactly once: {waits:?}"
        );
        // `> Duration::ZERO` は「非ゼロなら何でも通る」恒真に近い述語で、線形増加も
        // 上限への張り付きも守らない。取り違えたら別物になる具体値と突き合わせる。
        let expected: Vec<Duration> = (1..=PERSISTENT_FAILURES as u32)
            .map(|n| {
                ACCEPT_ERROR_BACKOFF_UNIT
                    .saturating_mul(n)
                    .min(ACCEPT_ERROR_BACKOFF_MAX)
            })
            .collect();
        assert_eq!(
            waits, expected,
            "backoff must grow linearly with consecutive failures and cap at ACCEPT_ERROR_BACKOFF_MAX: {waits:?}"
        );
    }

    /// 契約 §0 の手当てが「連続失敗回数」を数えていることの裏付け:
    /// 成功した接続を挟むと、直後の失敗の待機は積み上がった回数からではなく
    /// `UNIT`（1 回目相当）から数え直す。`UnixStream::pair()` で作った片方を
    /// `Ok` として `incoming` に混ぜ、相手側は即座に drop して `handle_connection`
    /// が空バッファで即 return するようにする。
    #[test]
    fn accept_loop_resets_backoff_after_a_successful_connection() {
        let (accepted, peer) = UnixStream::pair().expect("pair");
        drop(peer);

        let items: Vec<std::io::Result<UnixStream>> = vec![
            Err(std::io::Error::from_raw_os_error(libc::EMFILE)),
            Err(std::io::Error::from_raw_os_error(libc::EMFILE)),
            Ok(accepted),
            Err(std::io::Error::from_raw_os_error(libc::EMFILE)),
        ];

        let sink: Arc<dyn HookSink> = Arc::new(RecordingSink::default());
        let waits: Arc<Mutex<Vec<Duration>>> = Arc::new(Mutex::new(Vec::new()));
        let waits_for_sleep = Arc::clone(&waits);

        accept_loop_with(
            items.into_iter(),
            sink,
            Arc::new(AtomicBool::new(false)),
            move |d| waits_for_sleep.lock().expect("lock").push(d),
        );

        let waits = waits.lock().expect("lock").clone();
        // `Ok` 分岐は sleep を呼ばないので、記録は Err の 3 回分だけ。
        assert_eq!(
            waits,
            vec![
                ACCEPT_ERROR_BACKOFF_UNIT,
                ACCEPT_ERROR_BACKOFF_UNIT.saturating_mul(2),
                ACCEPT_ERROR_BACKOFF_UNIT,
            ],
            "a successful connection must reset consecutive_errors, not merely pause it: {waits:?}"
        );
    }

    #[test]
    fn resolves_relay_next_to_the_app_executable() {
        let dir = std::env::temp_dir().join(format!("kamux-relayres-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let relay = dir.join(RELAY_BIN_NAME);
        std::fs::write(&relay, b"#!/bin/sh\n").expect("write");

        let got = resolve_relay_bin_from(None, &dir).expect("resolve");
        assert_eq!(got, relay);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_override_wins_over_exe_dir() {
        let dir = std::env::temp_dir().join(format!("kamux-relayenv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let beside = dir.join(RELAY_BIN_NAME);
        std::fs::write(&beside, b"").expect("write");
        let custom = dir.join("custom-relay");
        std::fs::write(&custom, b"").expect("write");

        let got =
            resolve_relay_bin_from(Some(custom.to_str().expect("utf8")), &dir).expect("resolve");
        assert_eq!(got, custom);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_relay_is_cli_not_found() {
        let dir = std::env::temp_dir().join(format!("kamux-relaymiss-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");

        let err = resolve_relay_bin_from(None, &dir).expect_err("must fail");
        assert!(
            matches!(err, AppError::CliNotFound(_)),
            "unexpected error: {err:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_override_pointing_at_nothing_is_an_error() {
        let dir = std::env::temp_dir();
        let err =
            resolve_relay_bin_from(Some("/nonexistent/kamux-relay"), &dir).expect_err("must fail");
        let AppError::CliNotFound(msg) = &err else {
            panic!("unexpected error: {err:?}");
        };
        // variant の一致だけでは、beside 探索へのフォールスルーで別理由の
        // CliNotFound になっても見分けがつかない。メッセージに
        // KAMUX_RELAY_BIN が現れることまで見て、override 起因の失敗である
        // ことを固定する。
        assert!(
            msg.contains("KAMUX_RELAY_BIN"),
            "expected message to mention KAMUX_RELAY_BIN, got: {msg}"
        );
    }

    /// `resolve_relay_bin()`（実環境版）の配線を守る。§81.2 カテゴリ3: 同じ
    /// スコープに `exe`（実行ファイル自身）と `exe_dir`（その親ディレクトリ、
    /// `exe` から導出）という 2 本の `PathBuf` があり、`resolve_relay_bin_from`
    /// へ渡す際に取り違えても型検査は通る。`KAMUX_RELAY_BIN` の分岐は
    /// `exe_dir` に触れず早期 return するので、この配線はその分岐を経由する
    /// テストでは守れない —— ここでは「隣に置く」経路そのものを実行する。
    ///
    /// 注意: このテストはプロセス全体で共有される
    /// `current_exe().parent()/kamux-relay` というパスに書き込む。他のテストが
    /// 同じパスへ触れるようになったら競合しうる（現時点では本テストのみ）。
    #[test]
    fn resolve_relay_bin_looks_in_the_directory_containing_the_executable() {
        // KAMUX_RELAY_BIN が外から設定されていると env override 分岐が先に
        // 返り、exe_dir 経路を一度も通らない。その場合このテストは何も守ら
        // ないので、黙って緑になるのではなく前提として固定する。
        assert!(
            std::env::var_os("KAMUX_RELAY_BIN").is_none(),
            "this test observes the exe_dir branch; KAMUX_RELAY_BIN must not be set"
        );

        let exe = std::env::current_exe().expect("current_exe");
        let exe_dir = exe.parent().expect("parent").to_path_buf();
        let fixture = exe_dir.join(RELAY_BIN_NAME);
        std::fs::write(&fixture, b"").expect("write fixture");

        let got = resolve_relay_bin();

        let _ = std::fs::remove_file(&fixture);
        assert_eq!(got.expect("resolve"), fixture);
    }

    #[test]
    fn bootstrap_disables_hooks_when_relay_is_missing() {
        let dir = std::env::temp_dir().join(format!("kamux-boot-missing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");

        let sink = Arc::new(RecordingSink::default());
        let (runtime, server) = bootstrap_hooks_in(
            sink,
            Err(AppError::CliNotFound("relay missing".into())),
            hooks_socket_path().expect("sock"),
            dir.join("settings.json"),
        );

        assert!(
            runtime.is_none(),
            "hooks must be disabled when relay cannot be resolved"
        );
        assert!(server.is_none());
        assert!(!dir.join("settings.json").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bootstrap_enables_hooks_and_writes_settings() {
        let dir = std::env::temp_dir().join(format!("kamux-boot-ok-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let relay = dir.join(RELAY_BIN_NAME);
        std::fs::write(&relay, b"").expect("write relay");
        let sock = dir.join("boot.sock");
        let settings = dir.join("settings.json");

        let sink = Arc::new(RecordingSink::default());
        let (runtime, server) = bootstrap_hooks_in(
            sink.clone(),
            Ok(relay.clone()),
            sock.clone(),
            settings.clone(),
        );

        let runtime = runtime.expect("hooks must be enabled");
        assert_eq!(runtime.relay_bin, relay);
        assert_eq!(runtime.socket_path, sock);
        assert!(settings.exists(), "settings file must be written");
        assert!(sock.exists(), "socket must be listening");

        send(
            &sock,
            r#"{"v":1,"kamux_session_id":"3f2a0000-0000-4000-8000-000000009c1e","hook_kind":"Stop","payload":null}"#,
        );
        wait_for(|| sink.snapshot().len() == 1);

        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 契約 §96.2 の変異検証で見つかった実測の穴: `write_hook_settings_file` が
    /// 失敗したとき、`bootstrap_hooks_in` はソケットを既に起動してしまっている
    /// (`HooksServer::start` が先に走る) にもかかわらず、hooks を無効化して
    /// 呼び出し元へ `(None, None)` を返さなければならない。既存の 2 本の
    /// テストはどちらも settings の書き込みが成功する経路しか通っておらず、
    /// この分岐の early return を消す変異（`if let Err(e) = ... { warn!(..); }`
    /// から `return (None, None);` を落とす）を検出できなかった。
    #[test]
    fn bootstrap_disables_hooks_when_settings_write_fails() {
        let dir =
            std::env::temp_dir().join(format!("kamux-boot-settingsfail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let relay = dir.join(RELAY_BIN_NAME);
        std::fs::write(&relay, b"").expect("write relay");
        let sock = dir.join("boot.sock");
        // 親ディレクトリが存在しないパス。std::fs::write は NotFound で失敗する。
        let settings = dir.join("no-such-subdir").join("settings.json");

        let sink = Arc::new(RecordingSink::default());
        let (runtime, server) = bootstrap_hooks_in(sink, Ok(relay), sock.clone(), settings.clone());

        assert!(
            runtime.is_none(),
            "settings の書き込みに失敗したら hooks は無効化されなければならない"
        );
        assert!(server.is_none());
        assert!(!settings.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
