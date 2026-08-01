// src-tauri/src/pty/surface.rs
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtySize};

use crate::error::{AppError, AppResult};
use crate::pty::backpressure::{Backpressure, PTY_READ_CHUNK};

/// spawn 時の既定サイズ。フロントが attach 直後に fit() → resize_pty で合わせる
pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;

/// PTY の出力先。Tauri から切り離してテスト可能にするための境界
pub trait PtySink: Send + Sync + 'static {
    fn on_data(&self, surface_id: &str, base64: String, seq: u64);
    fn on_exit(&self, surface_id: &str, exit_code: Option<i32>);
}

/// PTY 起動の全パラメータ(契約 §15 の逐語)。
/// 契約は `src-tauri/src/pty/mod.rs` に置くと書いているが、実体はここに定義し
/// `pty/mod.rs` から `pub use surface::SpawnSpec;` で再エクスポートする
/// (`crate::pty::SpawnSpec` というパスは契約どおりに解決される)。
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub surface_id: String,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// PTY プロセスへ追加注入する環境変数。既存の環境は継承した上で上書きされる
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
}

/// 1 PTY = 1 サーフェス。reader / waiter の 2 スレッドを従える。
/// `master` / `writer` / `killer` は Task 4 (write/resize/kill) が読み出す消費者で、
/// Task 3 の時点では構築のみ(未読) のため `dead_code` を明示的に許可する。
pub struct PtySurface {
    id: String,
    #[allow(dead_code)]
    master: Mutex<Box<dyn MasterPty + Send>>,
    #[allow(dead_code)]
    writer: Mutex<Box<dyn Write + Send>>,
    #[allow(dead_code)]
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    backpressure: Arc<Backpressure>,
    alive: Arc<AtomicBool>,
}

// 内部フィールドはトレイトオブジェクト(MasterPty/Write/ChildKiller)を保持しており
// これらは Debug を実装しないため derive できない。テストの `expect_err` が
// `Result<Arc<PtySurface>, AppError>` に Debug を要求するため、id のみ出力する
// 最小限の手動実装を用意する。
impl std::fmt::Debug for PtySurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtySurface").field("id", &self.id).finish()
    }
}

/// 中毒したロックからも回復する(panic 経路を作らない)
#[allow(dead_code)] // Task 4 の write/resize/kill から使われる
fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl PtySurface {
    pub fn spawn(spec: SpawnSpec, sink: Arc<dyn PtySink>) -> AppResult<Arc<Self>> {
        let size = PtySize {
            rows: spec.rows.max(1),
            cols: spec.cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = native_pty_system()
            .openpty(size)
            .map_err(|e| AppError::PtySpawn(e.to_string()))?;

        let mut cmd = CommandBuilder::new(&spec.program);
        for arg in &spec.args {
            cmd.arg(arg);
        }
        cmd.cwd(&spec.cwd);
        // CommandBuilder は親環境を引き継ぐが、TERM は明示して端末種別を確定させる
        cmd.env("TERM", "xterm-256color");
        for (key, value) in &spec.env {
            cmd.env(key, value);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| AppError::PtySpawn(e.to_string()))?;
        // slave を落とさないと子プロセス終了後も master が EOF にならない
        drop(pair.slave);

        // child は spawn 済みなので、以降のどの失敗経路でも kill せずに return すると
        // 孤児プロセスとして残る。killer は child とは独立に kill を送れるため、
        // 以下の各エラー経路で確実に子プロセスを終わらせてから return する
        let mut killer = child.clone_killer();
        let reader = pair.master.try_clone_reader().map_err(|e| {
            let _ = killer.kill();
            AppError::PtySpawn(e.to_string())
        })?;
        let writer = pair.master.take_writer().map_err(|e| {
            let _ = killer.kill();
            AppError::PtySpawn(e.to_string())
        })?;

        let backpressure = Arc::new(Backpressure::new());
        let alive = Arc::new(AtomicBool::new(true));

        let reader_handle = match spawn_reader_thread(
            spec.surface_id.clone(),
            reader,
            Arc::clone(&backpressure),
            Arc::clone(&sink),
        ) {
            Ok(handle) => handle,
            Err(err) => {
                // reader スレッドの起動に失敗すると child はまだ誰にも渡っておらず、
                // このまま drop されると孤児プロセスとして残る
                let _ = killer.kill();
                return Err(err);
            }
        };

        if let Err(err) = spawn_waiter_thread(
            spec.surface_id.clone(),
            child,
            reader_handle,
            Arc::clone(&backpressure),
            Arc::clone(&alive),
            sink,
        ) {
            // waiter スレッドの起動に失敗すると、reader スレッドの JoinHandle は
            // ここで捨てられ二度と join されない。close() で滞留待ちの reader を
            // 起こし、killer で子プロセスも終わらせる
            // (終了経路は必ず close する契約。§「必ず満たせ」項目2)
            backpressure.close();
            let _ = killer.kill();
            return Err(err);
        }

        Ok(Arc::new(Self {
            id: spec.surface_id,
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            killer: Mutex::new(killer),
            backpressure,
            alive,
        }))
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub fn pending_bytes(&self) -> usize {
        self.backpressure.pending()
    }

    /// フロントが seq まで消化したことを反映する
    pub fn ack(&self, seq: u64) {
        self.backpressure.ack(seq);
    }
}

fn spawn_reader_thread(
    surface_id: String,
    mut reader: Box<dyn Read + Send>,
    backpressure: Arc<Backpressure>,
    sink: Arc<dyn PtySink>,
) -> AppResult<JoinHandle<()>> {
    std::thread::Builder::new()
        .name(format!("kamux-pty-read-{surface_id}"))
        .spawn(move || {
            let mut buf = vec![0u8; PTY_READ_CHUNK];
            loop {
                // 滞留が高水位を超えている間はここで眠る(ポーリングしない)
                if !backpressure.wait_until_drained() {
                    break;
                }
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let seq = backpressure.record(n);
                        sink.on_data(&surface_id, BASE64.encode(&buf[..n]), seq);
                    }
                    Err(err) if err.kind() == ErrorKind::Interrupted => continue,
                    // macOS では子プロセス終了時に read が EIO を返す
                    Err(_) => break,
                }
            }
        })
        .map_err(|e| AppError::Io(e.to_string()))
}

fn spawn_waiter_thread(
    surface_id: String,
    mut child: Box<dyn Child + Send + Sync>,
    reader_handle: JoinHandle<()>,
    backpressure: Arc<Backpressure>,
    alive: Arc<AtomicBool>,
    sink: Arc<dyn PtySink>,
) -> AppResult<()> {
    std::thread::Builder::new()
        .name(format!("kamux-pty-wait-{surface_id}"))
        .spawn(move || {
            let exit_code = child.wait().ok().map(|status| status.exit_code() as i32);
            alive.store(false, Ordering::SeqCst);
            // reader が滞留待ちで眠っていても起こす。close() を join() より先に
            // 呼ばないと、バックプレッシャーで停止した reader は二度と起きず
            // waiter の join() がハングする(契約: 終了経路は join の前に close)。
            backpressure.close();
            // 残りの出力を出し切ってから exit を通知する(表示順序の保証)
            let _ = reader_handle.join();
            sink.on_exit(&surface_id, exit_code);
        })
        .map(|_| ())
        .map_err(|e| AppError::Io(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD as BASE64;
    // `Engine` トレイトは production 側の `use base64::Engine as _;` が
    // `use super::*` 経由で既に持ち込んでいるため、ここでの再 import は不要
    // (`unused_imports` で `-D warnings` に引っかかる)
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::Arc;
    use std::time::Duration;

    #[derive(Debug)]
    pub(super) enum Ev {
        Data { base64: String, seq: u64 },
        Exit(Option<i32>),
    }

    pub(super) struct ChannelSink {
        pub tx: Sender<Ev>,
    }

    impl PtySink for ChannelSink {
        fn on_data(&self, _surface_id: &str, base64: String, seq: u64) {
            let _ = self.tx.send(Ev::Data { base64, seq });
        }
        fn on_exit(&self, _surface_id: &str, exit_code: Option<i32>) {
            let _ = self.tx.send(Ev::Exit(exit_code));
        }
    }

    pub(super) fn spec(program: &str, args: &[&str]) -> SpawnSpec {
        SpawnSpec {
            surface_id: "00000000-0000-4000-8000-000000000001:agent".to_string(),
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: std::path::PathBuf::from("/tmp"),
            env: Vec::new(),
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
        }
    }

    /// Exit を受け取るまでのデータを連結して返す。受け取った seq は都度 ack する
    pub(super) fn drain(rx: &Receiver<Ev>, surface: &PtySurface) -> (String, Option<i32>) {
        let mut out: Vec<u8> = Vec::new();
        loop {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(Ev::Data { base64, seq }) => {
                    out.extend_from_slice(&BASE64.decode(base64).expect("valid base64"));
                    surface.ack(seq);
                }
                Ok(Ev::Exit(code)) => return (String::from_utf8_lossy(&out).into_owned(), code),
                Err(err) => panic!("timed out waiting for pty events: {err}"),
            }
        }
    }

    #[test]
    fn echo_emits_its_output_then_exits_with_zero() {
        let (tx, rx) = channel();
        let surface = PtySurface::spawn(
            spec("/bin/echo", &["hello-kamux"]),
            Arc::new(ChannelSink { tx }),
        )
        .expect("spawn /bin/echo");
        let (out, code) = drain(&rx, &surface);
        assert!(out.contains("hello-kamux"), "actual output: {out:?}");
        assert_eq!(code, Some(0));
        assert!(!surface.is_alive());
    }

    #[test]
    fn spawn_fails_with_pty_spawn_error_for_missing_binary() {
        let (tx, _rx) = channel();
        let err = PtySurface::spawn(
            spec("/nonexistent/kamux-no-such-binary", &[]),
            Arc::new(ChannelSink { tx }),
        )
        .expect_err("must fail");
        assert!(
            matches!(err, AppError::PtySpawn(_)),
            "actual error: {err:?}"
        );
    }

    #[test]
    fn multibyte_output_survives_base64_round_trip() {
        let (tx, rx) = channel();
        let surface = PtySurface::spawn(
            spec("/bin/echo", &["あいうえお-🍣"]),
            Arc::new(ChannelSink { tx }),
        )
        .expect("spawn /bin/echo");
        let (out, _code) = drain(&rx, &surface);
        assert!(out.contains("あいうえお-🍣"), "actual output: {out:?}");
    }

    #[test]
    fn output_spanning_multiple_read_chunks_is_delivered_completely_before_exit() {
        // 「必ず満たせ」項目1・3 の弁別: PTY_READ_CHUNK(8KiB) を跨ぐ量の出力を
        // 継続的に ack しながら受信し、exit までに全行を欠落なく受け取れることを
        // 検証する。出力量(約85KB)は BACKPRESSURE_HIGH_WATER(1MiB)を大きく
        // 下回るよう選び、backpressure の一時停止とは独立に「複数チャンクの
        // 読み取り + 各チャンクへの seq 付与 + wait_until_drained のゲート」が
        // 正しく機能することだけを弁別する。
        //
        // 注記: 「reader が backpressure で真に停止した状態のまま子プロセスが
        // 終了する」経路(close()/join() の順序が効く境界ケース)は、PTY の
        // カーネル側フロー制御により、この状態を自然な大量出力だけで安全に
        // 再現しようとすると子プロセス自身が write() でブロックして終了しなく
        // なる(実験で確認済み)。Task 4 の kill() であれば SIGKILL で出力量に
        // 依存せず子プロセスを即座に終了させられるため、その決定的なテストは
        // Task 4 側に委ねる。
        let (tx, rx) = channel();
        let surface = PtySurface::spawn(
            spec("/bin/sh", &["-c", "yes 0123456789abcdef | head -n 5000"]),
            Arc::new(ChannelSink { tx }),
        )
        .expect("spawn /bin/sh");
        let (out, code) = drain(&rx, &surface);
        let lines = out.matches("0123456789abcdef").count();
        assert_eq!(
            lines,
            5000,
            "expected all 5000 lines, got {lines} (actual tail: {:?})",
            &out[out.len().saturating_sub(80)..]
        );
        assert_eq!(code, Some(0));
        assert!(!surface.is_alive());
    }

    /// `remaining` バイトを `PTY_READ_CHUNK` 単位で吐き出すだけの合成リーダー。
    /// 実プロセス・実 PTY のカーネル側フロー制御に依存せず、
    /// 「reader がバックプレッシャーのゲートで確実に停止している」状態を
    /// 決定的に作るために使う。
    struct FakeReader {
        remaining: usize,
    }

    impl std::io::Read for FakeReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.remaining == 0 {
                return Ok(0);
            }
            let n = buf.len().min(self.remaining);
            for byte in buf[..n].iter_mut() {
                *byte = b'a';
            }
            self.remaining -= n;
            Ok(n)
        }
    }

    #[derive(Debug)]
    struct FakeChildKiller;

    impl portable_pty::ChildKiller for FakeChildKiller {
        fn kill(&mut self) -> std::io::Result<()> {
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(FakeChildKiller)
        }
    }

    /// `exit_gate` に何か送られるまで `wait()` をブロックする合成子プロセス。
    /// テスト側が任意のタイミングで「子プロセスが終了した」を発火できるようにする。
    #[derive(Debug)]
    struct FakeChild {
        exit_gate: Mutex<Receiver<()>>,
    }

    impl portable_pty::ChildKiller for FakeChild {
        fn kill(&mut self) -> std::io::Result<()> {
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(FakeChildKiller)
        }
    }

    impl portable_pty::Child for FakeChild {
        fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
            Ok(None)
        }
        fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
            let _ = lock_or_recover(&self.exit_gate).recv();
            Ok(portable_pty::ExitStatus::with_exit_code(0))
        }
        fn process_id(&self) -> Option<u32> {
            None
        }
    }

    #[test]
    fn waiter_closes_backpressure_before_joining_a_reader_parked_on_high_water() {
        // 「必ず満たせ」項目2・3 の弁別テスト: reader がバックプレッシャーの
        // ゲート(wait_until_drained)で確実に停止している状態を合成の Read /
        // Child で決定的に作り、その状態で子プロセスの "終了" を発火させる。
        // waiter が Backpressure::close() を reader_handle.join() より先に
        // 呼んでいなければ、on_exit は既定の stall_timeout(5秒)が満了するまで
        // 届かない。実プロセスでこの境界を再現しようとすると、reader が
        // 止まった直後に子プロセス自身が write() でブロックして終了しなく
        // なるため(実験で確認済み)、ここではホワイトボックスに直接
        // spawn_reader_thread / spawn_waiter_thread を呼んで弁別する。
        use crate::pty::backpressure::BACKPRESSURE_HIGH_WATER;
        use std::time::Instant;

        let (tx, rx) = channel::<Ev>();
        let sink: Arc<dyn PtySink> = Arc::new(ChannelSink { tx });
        let backpressure = Arc::new(Backpressure::new());
        let alive = Arc::new(AtomicBool::new(true));

        let reader = FakeReader {
            remaining: BACKPRESSURE_HIGH_WATER + PTY_READ_CHUNK * 4,
        };
        let reader_handle = spawn_reader_thread(
            "surf".to_string(),
            Box::new(reader),
            Arc::clone(&backpressure),
            Arc::clone(&sink),
        )
        .expect("spawn reader thread");

        // reader が高水位を超えてゲートで停止する(次の read を試みなくなる)まで、
        // Data イベントを消費するだけで意図的に ack しない
        loop {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ev::Data { .. }) => {
                    if backpressure.pending() >= BACKPRESSURE_HIGH_WATER {
                        break;
                    }
                }
                other => panic!("unexpected event while pausing the reader: {other:?}"),
            }
        }
        assert!(backpressure.is_paused());

        let (exit_tx, exit_rx) = channel::<()>();
        let fake_child = FakeChild {
            exit_gate: Mutex::new(exit_rx),
        };
        spawn_waiter_thread(
            "surf".to_string(),
            Box::new(fake_child),
            reader_handle,
            Arc::clone(&backpressure),
            Arc::clone(&alive),
            sink,
        )
        .expect("spawn waiter thread");

        let started = Instant::now();
        exit_tx.send(()).expect("signal fake process exit");

        let event = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("on_exit が届かなかった: close() が join() より先に呼ばれていない疑いがある");
        let elapsed = started.elapsed();
        assert!(
            matches!(event, Ev::Exit(Some(0))),
            "actual event: {event:?}"
        );
        assert!(!alive.load(Ordering::SeqCst));
        assert!(
            elapsed < Duration::from_secs(2),
            "on_exit の到着が遅すぎる ({elapsed:?}): stall_timeout 待ちの疑いがある"
        );
    }

    #[test]
    fn ack_using_the_seq_on_data_actually_passed_unparks_reader_before_exit() {
        // Important A の弁別: `on_data` に渡す seq が `record` の戻り値そのもの
        // であることを検証する。もし seq が定数 0 や別カウンタに変異していると、
        // ここで捕まえた seq を ack しても Backpressure 内部の inflight は
        // 消化されず、park した reader は再開できない。
        //
        // 手順: 高水位を超えるまで Data を消費して park させる → `on_data` が
        // 実際に渡してきた最後の seq を ack する → reader が再開し、まだ Exit を
        // 発火していない waiter より先に「追加の Data」が届くことを確認する。
        // Exit は reader が完全に読み切って join() が返るまで送られない設計
        // (`waiter_closes_backpressure_before_joining_a_reader_parked_on_high_water`
        // が保証済み)なので、park から再開できたかどうかがこのテストの唯一の
        // 弁別点になる。
        use crate::pty::backpressure::BACKPRESSURE_HIGH_WATER;

        let (tx, rx) = channel::<Ev>();
        let sink: Arc<dyn PtySink> = Arc::new(ChannelSink { tx });
        let backpressure = Arc::new(Backpressure::new());
        let alive = Arc::new(AtomicBool::new(true));

        // park した後もまだ読み切っていないデータを残しておく(再開後の
        // 「追加の Data」を観測するため)
        let reader = FakeReader {
            remaining: BACKPRESSURE_HIGH_WATER + PTY_READ_CHUNK * 8,
        };
        let reader_handle = spawn_reader_thread(
            "surf".to_string(),
            Box::new(reader),
            Arc::clone(&backpressure),
            Arc::clone(&sink),
        )
        .expect("spawn reader thread");

        // park するまで Data を消費する。ack はせず、`on_data` が渡してきた最後の
        // seq を park 検知時点の値として受け取る
        let last_seq = loop {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ev::Data { seq, .. }) => {
                    if backpressure.pending() >= BACKPRESSURE_HIGH_WATER {
                        break seq;
                    }
                }
                other => panic!("unexpected event while pausing the reader: {other:?}"),
            }
        };
        assert!(backpressure.is_paused());

        let (exit_tx, exit_rx) = channel::<()>();
        let fake_child = FakeChild {
            exit_gate: Mutex::new(exit_rx),
        };
        spawn_waiter_thread(
            "surf".to_string(),
            Box::new(fake_child),
            reader_handle,
            Arc::clone(&backpressure),
            Arc::clone(&alive),
            sink,
        )
        .expect("spawn waiter thread");

        // `on_data` が実際に渡してきた seq で ack する
        backpressure.ack(last_seq);
        // reader が再開する猶予を与えてから子プロセスの終了を発火する。join() は
        // reader が読み切るまで返らないため、Exit は追加の Data より後にしか
        // 送られ得ない設計だが、ack が効いていなければ reader は park したままで
        // Exit は Data 無しに直接届く
        exit_tx.send(()).expect("signal fake process exit");

        let event = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("ack 後に何も届かなかった: on_data に渡す seq の配線が壊れている疑いがある");
        assert!(
            matches!(event, Ev::Data { .. }),
            "追加の Data より先に Exit(または何も無し)が届いた: seq の配線が \
             壊れている疑いがある (actual event: {event:?})"
        );

        // 後片付け: Exit まで読み飛ばしてスレッドを回収する
        loop {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ev::Exit(_)) => break,
                Ok(Ev::Data { .. }) => continue,
                Err(err) => panic!("timed out waiting for exit: {err}"),
            }
        }
    }

    #[test]
    fn spawn_starts_the_child_process_in_the_given_cwd() {
        // Important B の弁別: `cmd.cwd(&spec.cwd)` を削除する変異が生存しないことを
        // 確認するテスト。子プロセス自身に `pwd -P` を実行させ、実際の作業
        // ディレクトリを報告させる。macOS の /tmp は /private/tmp へのシンボリック
        // リンクなので、tempfile が返すパスをそのまま文字列比較するとフレークする
        // 恐れがある。`std::fs::canonicalize` で解決した値と比較する
        // (`pwd -P` も物理パスを返すため、両辺とも symlink 解決済みで揃う)。
        let (tx, rx) = channel();
        let dir = tempfile::tempdir().expect("create temp dir");
        let mut s = spec("/bin/sh", &["-c", "pwd -P"]);
        s.cwd = dir.path().to_path_buf();
        let surface = PtySurface::spawn(s, Arc::new(ChannelSink { tx })).expect("spawn /bin/sh");
        let (out, code) = drain(&rx, &surface);
        let expected = std::fs::canonicalize(dir.path())
            .expect("canonicalize temp dir")
            .to_string_lossy()
            .into_owned();
        assert_eq!(out.trim(), expected, "actual output: {out:?}");
        assert_eq!(code, Some(0));
    }

    #[test]
    fn spawn_creates_the_pty_with_the_given_cols_and_rows() {
        // Important B の弁別: `PtySize { cols, rows, .. }` の配線を定数
        // (DEFAULT_COLS/DEFAULT_ROWS = 80x24) に変異させる変異が生存しないことを
        // 確認するテスト。既定値と明確に異なる cols/rows を渡し、子プロセスの
        // `stty size`(macOS では "rows cols" の順で出力)がその値を報告する
        // ことを確認する。
        let (tx, rx) = channel();
        let mut s = spec("/bin/stty", &["size"]);
        s.cols = 100;
        s.rows = 40;
        assert_ne!(s.cols, DEFAULT_COLS);
        assert_ne!(s.rows, DEFAULT_ROWS);
        let surface = PtySurface::spawn(s, Arc::new(ChannelSink { tx })).expect("spawn /bin/stty");
        let (out, code) = drain(&rx, &surface);
        assert_eq!(out.trim(), "40 100", "actual output: {out:?}");
        assert_eq!(code, Some(0));
    }
}
