// src-tauri/src/pty/surface.rs
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

use crate::error::{AppError, AppResult};
use crate::pty::backpressure::{Backpressure, PTY_READ_CHUNK};

/// spawn 時の既定サイズ。フロントが attach 直後に fit() → resize_pty で合わせる
pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;

/// waiter が `Backpressure::close()` を撃った後、reader スレッドの `join()`
/// を待つ上限。
///
/// # なぜ「reader の自然な drain を待ってから close() する」設計(fix round 2)
/// を廃止したか
///
/// fix round 2 では、reader が最初の `read()` に到達する前に waiter が
/// `close()` を撃つと出力を丸ごと失いうる、という短命プロセスのバグに対して
/// 「reader が自然に読み切るまで close() を最大 500ms 遅らせる」対策を入れた。
/// しかしラウンド3のレビューで実測により、この対策が無効だったと判明した。
/// 真の機構は macOS の `ttywait`/`ttyclose` である: 最後の slave fd が閉じる
/// とき、カーネルは master 側が出力を読み切るまで `child.wait()` の復帰自体を
/// 最大 ~600ms ブロックし、それでも読み切られなければ未読バッファを破棄して
/// から `child.wait()` を返す。つまり `child.wait()` が返った時点で、出力が
/// 読めるかどうかの勝負は既にカーネル側でついている。waiter 側で close() を
/// 遅らせても、この破棄より後に効くタイマーでしかなく、無意味だった(実測:
/// reader に 3 秒の遅延を注入すると `child.wait()` は 603ms で復帰し、その後
/// reader が実際に `read()` して 0 バイトを得ることを確認した)。
///
/// そのため close() は再び `child.wait()` 復帰直後に即座に撃つ(fix round 1
/// 以前と同じ)。短命プロセスの出力全損そのものは直さない(根治には reader を
/// 子より先に起動する `spawn` の再設計が要るが、park している最中に子が
/// 終了すれば ttywait は必ずタイムアウトするため park ケースには効かず、
/// lane-controller の裁定で受容されている)。
///
/// # このチャネルの新しい役割: bounded join
///
/// `close()` は 2 つのケースで reader に効く:
/// - reader が高水位でゲート(`wait_until_drained`)に park している場合:
///   close() は条件変数を起こすので、reader はほぼ即座にループを抜ける
/// - reader が `read()` でブロックしている場合: 子プロセスが本当に終了して
///   いれば(=孫プロセスが slave を握っていなければ)EOF/EIO がすぐ返る
///
/// このどちらのケースでも、reader スレッド終了時に drop される `drain_tx`
/// (mpsc::Sender)が、waiter 側の `drain_rx.recv_timeout()` にほぼ即座に
/// `Disconnected` を返す。`PTY_JOIN_DEADLINE` はその十分な上界として 1 秒とした。
/// これを超えるのは、孫プロセスが setsid 済みの pgid に残って slave を握り
/// 続け、reader の `read()` が本当に無期限にブロックしている(wedge している)
/// 場合だけである(例: `$SHELL -l` で `sleep 1000 &` してから `exit` した
/// ケース。契約 §9 が最悪のケースとして名指ししている経路)。この場合
/// `join()` を無条件に待ち続けると `pty://exit` が永久に飛ばず、M2-1 の
/// 状態機械が `exited` へ遷移できずカードが永久に 🟢 のまま残ってしまう
/// (pty://exit が飛ばないほうが join() のタイムアウトより桁違いに悪い)。
/// そのため waiter は `PTY_JOIN_DEADLINE` でタイムアウトしたら、reader
/// スレッドをリークさせてでも(`JoinHandle` を join せずに drop するだけ。
/// 検知不能のままスレッドが走り続けるだけで panic やリソース破壊は起きない)
/// `on_exit` を優先して届ける。
///
/// # この Timeout 分岐に限られるトレードオフ: `on_exit` の後に `on_data` が
/// 最大 1 件届きうる
///
/// Timeout 分岐で `on_exit` を撃って `return` した後も、リークした reader
/// スレッドは走り続けている。その `read()` が(孫プロセス側の出力等で)
/// たまたま `n > 0` を得て戻ってくると、reader は
/// `backpressure.record(n)` → `sink.on_data(...)` を実行してから、ループ
/// 先頭の `wait_until_drained()` で `closed` を見て初めて終了する。
/// `Backpressure::record()` は `closed` を見ずに `seq` を進めるため、この
/// 経路では **`pty://exit` の後に `pty://data` が最大 1 件届き**、かつ
/// **論理的には既に死んでいる surface の `seq` が 1 つ進む**ことがある。
/// 素の `join()` だった(Timeout 分岐を持たない)修正前は、この順序は
/// 構造的に起こり得なかった。この挙動を変える(reader が `closed` を見て
/// emit 自体を抑止する等)かどうかは M2-1 との契約に関わる判断のため、
/// ここでは事実を記録するに留め、コードは変更していない。
///
/// 契約に定義は無い内部実装用の定数(契約 §15 は M1-3 が内部実装を自由に
/// 決めてよいと明記している)。
const PTY_JOIN_DEADLINE: Duration = Duration::from_secs(1);

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
pub struct PtySurface {
    id: String,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    /// `kill()` が SIGKILL を直接送るのに使う。portable-pty の
    /// `ChildKiller`(`clone_killer()` が返す `ProcessSignaller`)は unix では
    /// SIGHUP を1発送るだけでエスカレーションが無いため使わない
    /// (詳細は `kill()` のドキュメントコメント参照)。
    ///
    /// `Mutex` で包んでいるのは値の排他制御のためだけではない。waiter スレッドは
    /// `child.wait()` で reap した直後に、この Mutex を保持したまま `None` を
    /// 書き込む。`kill()` 側も同じ Mutex を保持したまま「pid が Some か」の
    /// チェックとシグナル送出の syscall を行うため、両者は互いに排他される。
    /// これにより「reap 済みで OS が pid を再利用しうる状態」と「kill() が
    /// その pid へシグナルを送る」が同時に起こる窓は、`wait()` 復帰〜ロック取得
    /// までの短い区間に縮小される(完全には消えない。TOCTOU 対策)。
    /// waiter スレッドも同じ `Arc` を共有するため `Arc` で包む。
    pid: Arc<Mutex<Option<u32>>>,
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
        // COLORTERM は truecolor 対応を宣言する。所有はここ(契約 §60.6.2)。
        // nvim / claude / codex いずれの TUI も同じ端末に描くため全 surface に適用する
        cmd.env("COLORTERM", "truecolor");
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
        // (成功経路では使わない。成功後の kill() は SIGKILL を直接送る。詳細は
        // `kill()` のドキュメントコメント参照)
        let mut killer = child.clone_killer();
        // 成功経路で `PtySurface::kill()` が使う。waiter スレッドへ `child` を
        // 渡す(move する)前に取得しておく必要がある。waiter スレッドと
        // `PtySurface` 本体の双方が同じ Mutex を共有する必要があるため、
        // `Self` を構築するより前に `Arc<Mutex<_>>` として確保しておく
        let pid = Arc::new(Mutex::new(child.process_id()));
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
        // reader スレッドが自然に終了(EOF/EIO)すると drain_tx が drop され、
        // waiter 側の drain_rx.recv_timeout() が即座に Disconnected を返す。
        // 詳細は spawn_waiter_thread のコメント参照
        let (drain_tx, drain_rx) = std::sync::mpsc::channel::<()>();

        let reader_handle = match spawn_reader_thread(
            spec.surface_id.clone(),
            reader,
            Arc::clone(&backpressure),
            Arc::clone(&sink),
            drain_tx,
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
            Arc::clone(&pid),
            sink,
            drain_rx,
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
            pid,
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

    /// PTY にバイト列を書き込む
    pub fn write(&self, data: &[u8]) -> AppResult<()> {
        let mut writer = lock_or_recover(&self.writer);
        writer
            .write_all(data)
            .map_err(|e| AppError::Io(e.to_string()))?;
        writer.flush().map_err(|e| AppError::Io(e.to_string()))
    }

    /// 端末サイズを変更する。0 は子プロセスを壊すので 1 にクランプする
    pub fn resize(&self, cols: u16, rows: u16) -> AppResult<()> {
        let master = lock_or_recover(&self.master);
        master
            .resize(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| AppError::Io(e.to_string()))
    }

    pub fn size(&self) -> AppResult<(u16, u16)> {
        let master = lock_or_recover(&self.master);
        let size = master.get_size().map_err(|e| AppError::Io(e.to_string()))?;
        Ok((size.cols, size.rows))
    }

    /// 子プロセスを殺す。契約 §15: SIGKILL 相当、完全に冪等(既に死んでいても Ok)。
    ///
    /// portable-pty 0.9 の `ChildKiller::kill`(`clone_killer()` が返す
    /// `ProcessSignaller`)は unix では SIGHUP を1発送るだけでエスカレーションが
    /// 無く、SIGHUP をハンドル/無視するプロセスを終了させられない。grace
    /// period → SIGKILL のエスカレーションを持つのは
    /// `impl ChildKiller for std::process::Child` だけだが、その `Child` は
    /// waiter スレッドが `wait()` のために専有しており、ここから呼べない。
    /// そのため spawn 時に取得した PID へ直接 `SIGKILL` を送る。子は
    /// portable-pty の unix 実装が `pre_exec` で `setsid()` しており
    /// プロセスグループリーダー(pgid == pid)になっているため、負の pid で
    /// `kill(2)` を呼ぶと `killpg` 相当になり、子が生んだ孫プロセスも
    /// まとめて終わらせられる。
    ///
    /// pid 再利用対策(TOCTOU): 「pid が Some か確認する」から「実際に
    /// syscall を撃つ」までを `pid` の Mutex ガード 1 回の下で行う。waiter
    /// スレッドは `child.wait()` で reap した直後、同じ Mutex を保持したまま
    /// `None` を書き込む。この 2 者が同じ Mutex で直列化されるため、
    /// 「reap 済みで OS が pid を再利用しうる状態」で本関数がまだ古い pid へ
    /// シグナルを送ってしまう窓は、`wait()` 復帰〜ロック取得までの短い区間に
    /// 縮小される(完全には消えない)。
    pub fn kill(&self) -> AppResult<()> {
        // reader が滞留待ちで停止していても必ず起こす。close() は何度呼んでも安全
        self.backpressure.close();
        let guard = lock_or_recover(&self.pid);
        let Some(pid) = *guard else {
            // None は「process_id() が取れなかった」か「waiter が既に reap 済み」
            // のいずれか。どちらもこれ以上シグナルを送る手段/必要が無い
            return Ok(());
        };
        // u32 → i32 変換に失敗するのは pid が i32::MAX を超える場合のみで、
        // macOS の pid_max(既定・上限とも i32 の範囲内)では到達不能。単項マイナス
        // (`-(pid as libc::pid_t)`)は pid が `i32::MIN` にキャストされると
        // debug ビルドで overflow panic するため、`try_from` で明示的に弾き、
        // 万一到達した場合もシグナルを送らず Ok を返す(Drop 経由で呼ばれても
        // panic しない)。
        let Ok(pid_t) = i32::try_from(pid) else {
            return Ok(());
        };
        // Safety: `guard` を保持したままここに到達している。waiter スレッドは
        // `child.wait()` から復帰した直後、この同じ Mutex を保持してからでない
        // と `pid` を `None` に書き換えられない(上のコメント参照)。つまり
        // このロックを握っている間、waiter はまだ pid を書き換え中でないことが
        // 保証され、「reap 済みで OS が pid を再利用した後の別プロセス
        // (グループ)を誤って殺す」という論理ハザードの窓は、`wait()` 復帰〜
        // ロック取得までの短い区間に縮小される(完全には消えない)。
        // `libc::kill` 自体は任意の整数値に対してメモリ安全な呼び出しであり、
        // 危険なのは UB ではなくこの論理ハザードの方である。
        let result = unsafe { libc::kill(-pid_t, libc::SIGKILL) };
        drop(guard);
        if result == 0 {
            return Ok(());
        }
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            // ESRCH/EPERM いずれも「シグナルできる相手が 1 つも無かった」を
            // 意味し、契約 §15 の冪等性で Ok に落とすべきケース。根拠は
            // is_nothing_left_to_signal() のドキュメントコメント参照。
            Some(errno) if is_nothing_left_to_signal(errno) => Ok(()),
            _ => Err(AppError::Io(err.to_string())),
        }
    }
}

/// 対象(プロセス/プロセスグループ)へ「シグナルを届けられる相手が 1 つも
/// 残っていない」という**状態**を判定する。契約 §15: `PtySurface::kill()`
/// は完全に冪等でなければならない。正典はこの**状態の集合**であり
/// (契約 §98.2 の規則 K。状態 4「終了処理中・未 reap」と状態 5「reap 済み
/// だが `pid` がまだ `None` になっていない」が該当する)、errno はその状態の
/// **帰結**にすぎない。
///
/// 以下は macOS でその状態が観測されたときに実際に返ってくる errno の
/// **列挙であって上限ではない**。同じ状態に対応する別の errno が判明すれば
/// (契約 §98.7 のとおり本挙動は macOS 固有で POSIX の保証ではない)、この
/// match 式に追加する。
///
/// - `ESRCH`: 既に reap 済みで、プロセス(グループ)自体がもう存在しない
///   状態(状態 5)に対応する。
/// - `EPERM`: macOS の `kill(-pgid, sig)` はプロセスグループ宛のシグナル
///   送信で「シグナルできたメンバが 0」のとき、`ESRCH` ではなく `EPERM` を
///   返す。子が SIGKILL を受けて終了処理に入った時点(reap のおよそ
///   25〜200µs 前。実 PTY で未読出力が残っていると waiter 側の `ttywait`
///   により最大 ~601ms 前まで広がる)から、waiter が `waitpid` で reap する
///   までの区間(状態 4)がこれに当たる(実測: 契約 §98.1 の表 行1・2・4)。
///   同じプロセスグループに生存中の別メンバが居る場合は、1 つでも殺せれば
///   `rc=0` になる(同調査 `mixed.c` での実測)ため、`EPERM` は「一部だけ
///   殺せた中途半端な状態」を隠さない —— 文字通り 1 つもシグナルが
///   届かなかった場合にのみ返る(契約 §98.3)。kamux は子プロセスを自分自身
///   で spawn しており同一 uid のため、この経路で「本物の権限エラー(殺せる
///   相手がいるのに殺せない)」が起きることは実質的に無い。
fn is_nothing_left_to_signal(errno: i32) -> bool {
    matches!(errno, libc::ESRCH | libc::EPERM)
}

impl Drop for PtySurface {
    fn drop(&mut self) {
        // `master`/`writer` フィールドを drop するだけでは、SIGHUP を
        // ハンドル/無視するよう構成された子プロセス(例: `trap '' HUP`)を
        // 終了させられない(実測: Drop 未実装のまま
        // `dropping_a_surface_without_kill_still_terminates_the_child_process`
        // 相当の状況を再現すると、5 秒待っても生存し続けることを確認済み)。
        // kill() で確実に子プロセスを終わらせ、backpressure を close して
        // 滞留待ちの reader も起こす(kill() 自体が close() を呼ぶ)。
        // 既に kill 済み/自然終了済みでも kill() は Ok を返すため panic しない。
        let _ = self.kill();
    }
}

fn spawn_reader_thread(
    surface_id: String,
    mut reader: Box<dyn Read + Send>,
    backpressure: Arc<Backpressure>,
    sink: Arc<dyn PtySink>,
    drain_tx: Sender<()>,
) -> AppResult<JoinHandle<()>> {
    std::thread::Builder::new()
        .name(format!("kamux-pty-read-{surface_id}"))
        .spawn(move || {
            // ループ全体で保持し、スレッド終了時(このクロージャを抜けるとき)に
            // drop される。waiter 側はこの drop(Sender 側切断 = Disconnected)
            // を「reader が自然に読み切って終わった」合図として使う
            // (詳細は spawn_waiter_thread のコメント参照)。
            let _drain_tx = drain_tx;
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

// 指摘A修正で `drain_rx` を追加したことで 7→8 引数になった。全て異なる型で
// 意味も独立しており(構造体1個にまとめると、テストの白箱呼び出し側で
// 「どのフィールドが何を表すか」を毎回書き起こす手間が増えるだけで
// 可読性は上がらないと判断)、この private ヘルパー限定で許可する。
#[allow(clippy::too_many_arguments)]
fn spawn_waiter_thread(
    surface_id: String,
    mut child: Box<dyn Child + Send + Sync>,
    reader_handle: JoinHandle<()>,
    backpressure: Arc<Backpressure>,
    alive: Arc<AtomicBool>,
    pid: Arc<Mutex<Option<u32>>>,
    sink: Arc<dyn PtySink>,
    drain_rx: Receiver<()>,
) -> AppResult<()> {
    std::thread::Builder::new()
        .name(format!("kamux-pty-wait-{surface_id}"))
        .spawn(move || {
            let exit_code = child.wait().ok().map(|status| status.exit_code() as i32);
            // reap した直後、Mutex を保持したまま pid を None にする。`kill()`
            // 側もこの同じ Mutex を保持したまま「pid が Some か」のチェックと
            // シグナル送出を行うため、この 1 行が pid 再利用 TOCTOU を塞ぐ境界
            // になる(詳細は `PtySurface::kill()` のドキュメントコメント参照)。
            *lock_or_recover(&pid) = None;
            alive.store(false, Ordering::SeqCst);
            // close() は child.wait() 復帰直後に即座に撃つ。`child.wait()` が
            // 返った時点で「出力が読めるかどうか」の勝負は既に macOS の
            // ttywait/ttyclose で決着済みなので、ここで close() を遅らせても
            // 意味が無い(詳細は PTY_JOIN_DEADLINE のドキュメントコメント参照)。
            // 滞留待ちで眠っている reader を起こす役目もこれが担う。
            backpressure.close();
            // reader が自然に終了(EOF/EIO、または上の close() を受けての
            // park 解除)すれば drain_tx が drop され、ここは即座に
            // Disconnected を返す。reader が孫プロセスに slave を握られて
            // read() で本当に無期限にブロックしている(wedge している)場合
            // だけ、ここは PTY_JOIN_DEADLINE でタイムアウトする。
            match drain_rx.recv_timeout(PTY_JOIN_DEADLINE) {
                Err(RecvTimeoutError::Timeout) => {
                    // join() を無条件に待ち続けると pty://exit が永久に飛ばない
                    // (契約 §9 が最悪のケースとして名指しした経路: 孫プロセスが
                    // slave を握ったまま親が exit するケース)。reader スレッド
                    // を JoinHandle ごと drop してリークさせてでも(検知不能の
                    // まま走り続けるだけで panic やリソース破壊は起きない)
                    // on_exit を優先して届ける。
                    // この分岐に限り、リークした reader が read() から復帰した
                    // 場合 on_exit の後に on_data が最大 1 件届き、死んだ
                    // surface の seq が 1 つ進みうる(詳細は PTY_JOIN_DEADLINE
                    // のドキュメントコメント参照。挙動は未変更、記録のみ)
                    sink.on_exit(&surface_id, exit_code);
                    return;
                }
                _ => {
                    // Disconnected: reader スレッドが終了して drain_tx が
                    // drop された(= close() が期待どおり効いた)。
                    // Ok(()) の場合は現状 reader が drain_tx へ send しない
                    // ため到達しないが、将来 send する実装に変わっても
                    // 「reader がまだ生きている可能性がある」前提で join()
                    // に進むのが安全なため、ここでは分岐しない
                }
            }
            // reader は既に終了しているはずなので、この join() は速やかに
            // 返る。残りの出力を出し切ってから exit を通知する(表示順序の保証)
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

    /// `predicate` が真になるまで Data を蓄積して ack し続け、蓄積した文字列を
    /// 返す。子プロセスを生かしたまま観測するための共通ヘルパー。
    ///
    /// # なぜこの形にするか(fix round 1)
    ///
    /// 「短命プロセスを spawn して、その出力内容をアサートする」形のテストは
    /// macOS の ttywait/ttyclose により構造的にフレークする: 最後の slave fd
    /// が閉じるとき、カーネルは master 側が読み切るまで子プロセスの終了自体を
    /// 最大 ~600ms ブロックし、それでも読み切られなければ未読バッファを
    /// 破棄してから `child.wait()` が返る(詳細は `PTY_JOIN_DEADLINE` の
    /// ドキュメントコメント参照)。この欠陥自体は lane-controller の裁定で
    /// 受容済みであり、直すのは「テストの作り方」だけである。子プロセスを
    /// 生かしたまま出力を観測し終えてから `kill()` すれば、この欠陥の窓に
    /// 構造的に入らない。
    ///
    /// 待ちは `recv_timeout` を Data 受信のたびにリセットし続けるだけだと
    /// 総経過時間に上限が無いため、`overall_timeout` で総経過時間にも
    /// 別枠で上限を設け、超えたら明示的に panic する。
    pub(super) fn observe_while_alive(
        rx: &Receiver<Ev>,
        surface: &PtySurface,
        overall_timeout: Duration,
        mut predicate: impl FnMut(&str) -> bool,
    ) -> String {
        let started = std::time::Instant::now();
        let mut out = String::new();
        loop {
            if predicate(&out) {
                return out;
            }
            if started.elapsed() >= overall_timeout {
                panic!(
                    "timed out after {overall_timeout:?} waiting for expected output; \
                     actual output so far: {out:?}"
                );
            }
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(Ev::Data { base64, seq }) => {
                    out.push_str(&String::from_utf8_lossy(
                        &BASE64.decode(base64).expect("valid base64"),
                    ));
                    surface.ack(seq);
                }
                Ok(Ev::Exit(code)) => panic!(
                    "child exited (code={code:?}) before expected output arrived; \
                     actual output: {out:?}"
                ),
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => panic!(
                    "channel disconnected before expected output arrived; actual output: {out:?}"
                ),
            }
        }
    }

    /// `kill()` 済みの surface から Exit イベントが届くまで Data を読み飛ばして
    /// ack する。SIGKILL 後なので exit code はアサートしない(契約: kill() で
    /// 終了させたテストで exit_code == Some(0) をアサートしてはならない)。
    ///
    /// # 総経過時間の上限(fix round 2, 指摘2)
    ///
    /// `rx.recv_timeout(Duration::from_secs(10))` を `Ev::Data` 受信のたびに
    /// 再スタートするだけの構造だと、指示が明示的に禁止した「recv_timeout を
    /// Data 受信のたびにリセットし続ける」構造そのものになってしまう。ここが
    /// ブロッキングしない理由は、`spawn_waiter_thread` が `child.wait()`
    /// 復帰後の bounded join(`PTY_JOIN_DEADLINE` = 1 秒)を経て必ず `on_exit`
    /// を送出する設計であり、`Ev::Exit` は必ず到達してループが必ず終わる
    /// ためである。その保証が万一崩れた場合にテストが無期限にハングし続ける
    /// のを防ぐため、`observe_while_alive` と同様に総経過時間にも別枠で上限を
    /// 設け、超えたら明示的に panic する。上限は「`kill()` 後に `on_exit` が
    /// 届くまでの最大は `PTY_JOIN_DEADLINE`(1 秒)+ reap 時間(実測フロアは
    /// macOS の ttywait による ~600ms 程度)」に十分な余裕を持たせて 30 秒とする。
    pub(super) fn drain_until_exit_after_kill(rx: &Receiver<Ev>, surface: &PtySurface) {
        const OVERALL_TIMEOUT: Duration = Duration::from_secs(30);
        let started = std::time::Instant::now();
        loop {
            // 残り予算を毎回計算して `recv_timeout` に渡す。固定 10 秒を渡すと
            // 「総経過時間 29.9 秒地点でチェックを通過 → 新たに 10 秒待つ」が
            // 起こりうる。これを放置すると実際のブロック上限が総経過時間の
            // 上限(30 秒)より大きくなって panic メッセージと矛盾する。
            // 残り予算でキャップすることで、実際のブロック時間を総経過時間の
            // 上限内に収める。
            let remaining = OVERALL_TIMEOUT.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                panic!(
                    "timed out after {OVERALL_TIMEOUT:?} (total elapsed) waiting for exit \
                     after kill: on_exit never arrived even though recv_timeout kept being \
                     reset by incoming Data events"
                );
            }
            match rx.recv_timeout(remaining.min(Duration::from_secs(10))) {
                Ok(Ev::Data { seq, .. }) => surface.ack(seq),
                Ok(Ev::Exit(_)) => break,
                Err(err) => panic!("timed out waiting for exit after kill: {err}"),
            }
        }
    }

    #[test]
    fn echo_emits_its_output_while_the_child_stays_alive() {
        // fix round 1: 子プロセスを生かしたまま観測してから kill() する形に
        // 変更(理由は observe_while_alive のドキュメントコメント参照)。
        // `/bin/echo` 単体だと直後に自然終了してしまうため、`/bin/sh -c` で
        // 出力の後に `sleep 30` を残す。
        let (tx, rx) = channel();
        let surface = PtySurface::spawn(
            spec("/bin/sh", &["-c", "printf '%s\\n' 'hello-kamux'; sleep 30"]),
            Arc::new(ChannelSink { tx }),
        )
        .expect("spawn /bin/sh");
        let out = observe_while_alive(&rx, &surface, Duration::from_secs(10), |acc| {
            acc.contains("hello-kamux")
        });
        assert!(out.contains("hello-kamux"), "actual output: {out:?}");
        surface.kill().expect("kill");
        drain_until_exit_after_kill(&rx, &surface);
        assert!(!surface.is_alive());
    }

    #[test]
    fn spawn_reports_exit_code_zero_when_the_child_exits_successfully() {
        // fix round 1: exit code の到達だけを検証する(出力内容は見ない)。
        // 出力を伴わない即終了プロセスなので、ttywait/ttyclose が破棄しうる
        // 「内容」自体が存在せず、短命プロセスのままでも構造的にフレークしない
        // (破棄されるのは未読の出力バッファであって、child.wait() が返す
        // exit code そのものではない)。
        let (tx, rx) = channel();
        let surface = PtySurface::spawn(
            spec("/bin/sh", &["-c", "exit 0"]),
            Arc::new(ChannelSink { tx }),
        )
        .expect("spawn /bin/sh");
        let (_out, code) = drain(&rx, &surface);
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
        // fix round 1: 子プロセスを生かしたまま観測してから kill() する形に
        // 変更(理由は observe_while_alive のドキュメントコメント参照)。
        let (tx, rx) = channel();
        let surface = PtySurface::spawn(
            spec(
                "/bin/sh",
                &["-c", "printf '%s\\n' 'あいうえお-🍣'; sleep 30"],
            ),
            Arc::new(ChannelSink { tx }),
        )
        .expect("spawn /bin/sh");
        let out = observe_while_alive(&rx, &surface, Duration::from_secs(10), |acc| {
            acc.contains("あいうえお-🍣")
        });
        assert!(out.contains("あいうえお-🍣"), "actual output: {out:?}");
        surface.kill().expect("kill");
        drain_until_exit_after_kill(&rx, &surface);
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
        //
        // fix round 1: `head -n 5000` の後ろに `sleep 30` を残し、5000 行が
        // 全て届いた時点で観測を打ち切って kill() する形に変更(理由は
        // observe_while_alive のドキュメントコメント参照)。「複数チャンクに
        // またがる出力が完全に届く」という検証意図(5000 行の完全一致)は
        // 弱めていない。
        let (tx, rx) = channel();
        let surface = PtySurface::spawn(
            spec(
                "/bin/sh",
                &["-c", "yes 0123456789abcdef | head -n 5000; sleep 30"],
            ),
            Arc::new(ChannelSink { tx }),
        )
        .expect("spawn /bin/sh");
        let out = observe_while_alive(&rx, &surface, Duration::from_secs(20), |acc| {
            acc.matches("0123456789abcdef").count() >= 5000
        });
        let lines = out.matches("0123456789abcdef").count();
        assert_eq!(
            lines,
            5000,
            "expected all 5000 lines, got {lines} (actual tail: {:?})",
            &out[out.len().saturating_sub(80)..]
        );
        surface.kill().expect("kill");
        drain_until_exit_after_kill(&rx, &surface);
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
    fn waiter_wakes_a_reader_parked_on_high_water_and_still_delivers_on_exit_promptly() {
        // Fix round 3 で復活させた Task 3 由来の不変条件: reader がバック
        // プレッシャーのゲート(wait_until_drained)で確実に停止している状態を
        // 合成の Read / Child で決定的に作り、その状態で子プロセスの "終了" を
        // 発火させる。close() は「reader が自然に drain するのを待つ」フェーズ
        // (fix round 2)を経由せず、child.wait() 復帰直後に即座に撃つ(fix
        // round 3 で復元した挙動。理由は PTY_JOIN_DEADLINE のドキュメント
        // コメント参照: 実測でこの「待ち」は既にカーネル側の ttywait/ttyclose
        // で勝負がついた後に始まるタイマーだったと判明したため)。park した
        // reader は close() の通知でほぼ即座にゲートを抜けるため、on_exit の
        // 到着は「reader が drain 完了を自ら知らせるまで待つ」旧設計での
        // 所要時間(常に旧 deadline の 500ms 前後)よりずっと短くなるはずである。
        // 実プロセスでこの境界を再現しようとすると、reader が止まった直後に
        // 子プロセス自身が write() でブロックして終了しなくなるため(実験で
        // 確認済み)、ここではホワイトボックスに直接 spawn_reader_thread /
        // spawn_waiter_thread を呼んで弁別する。
        use crate::pty::backpressure::BACKPRESSURE_HIGH_WATER;
        use std::time::Instant;

        let (tx, rx) = channel::<Ev>();
        let sink: Arc<dyn PtySink> = Arc::new(ChannelSink { tx });
        let backpressure = Arc::new(Backpressure::new());
        let alive = Arc::new(AtomicBool::new(true));

        let reader = FakeReader {
            remaining: BACKPRESSURE_HIGH_WATER + PTY_READ_CHUNK * 4,
        };
        let (drain_tx, drain_rx) = channel::<()>();
        let reader_handle = spawn_reader_thread(
            "surf".to_string(),
            Box::new(reader),
            Arc::clone(&backpressure),
            Arc::clone(&sink),
            drain_tx,
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
        let pid = Arc::new(Mutex::new(None));
        spawn_waiter_thread(
            "surf".to_string(),
            Box::new(fake_child),
            reader_handle,
            Arc::clone(&backpressure),
            Arc::clone(&alive),
            pid,
            sink,
            drain_rx,
        )
        .expect("spawn waiter thread");

        let started = Instant::now();
        exit_tx.send(()).expect("signal fake process exit");

        // park 検知ループは Data を1個ずつ ack せずに消費するため、break 時点で
        // channel に未消費の Data が複数残っていることがある(producer の reader
        // スレッドが先行送出できるため)。exit_tx.send() 後の受信が「残留した
        // 古い Data」を拾ってしまわないよう、Exit が届くまで Data を読み飛ばす。
        // Exit が届かなければタイムアウトして panic するため、
        // 「close() が最終的に join() より先に呼ばれる」という本来の弁別は
        // 弱めていない。
        let event = loop {
            match rx.recv_timeout(Duration::from_secs(3)) {
                Ok(Ev::Data { .. }) => continue,
                Ok(ev) => break ev,
                Err(err) => panic!(
                    "on_exit が届かなかった: close() が join() より先に park した reader を \
                     起こしていない疑いがある ({err})"
                ),
            }
        };
        let elapsed = started.elapsed();
        assert!(
            matches!(event, Ev::Exit(Some(0))),
            "actual event: {event:?}"
        );
        assert!(!alive.load(Ordering::SeqCst));
        // 400ms は旧設計(reader が drain 完了を自ら知らせるまで close() を
        // 遅らせる、常に旧 deadline の 500ms を要した)と新設計(close() が
        // 即座 → park した reader がほぼ即座に起きる)を区別するための閾値。
        // 500ms より確実に小さく、かつ通常のスケジューリング揺らぎを吸収できる
        // 十分な余裕を持たせた
        assert!(
            elapsed < Duration::from_millis(400),
            "on_exit の到着が遅すぎる ({elapsed:?}): close() が park した reader を \
             即座に起こせていない(旧設計の「drain 完了を待つ」経路に戻っている)疑いがある"
        );
    }

    #[test]
    fn waiter_still_emits_on_exit_within_the_join_deadline_when_reader_is_wedged_on_read() {
        // 修正2の核心(契約 §9 が最悪と名指しした経路): `$SHELL -l` で
        // `sleep 1000 &` してから `exit` すると、孫プロセスは setsid 済みの
        // pgid に残って slave を握り続け、reader は無出力の read() で
        // 永久にブロックしうる。この状態を合成の Read で決定的に作る
        // (対応する Sender をテスト側で意図的に drop しない = read() が
        // 永久に返らない状態を模す)。waiter は reader_handle.join() を
        // 無条件に待たず、PTY_JOIN_DEADLINE でタイムアウトしたら reader
        // スレッドをリークさせてでも on_exit を届けなければならない。
        struct WedgedReader {
            block: Receiver<()>,
            // reader が read() に実際に到達したことをテスト側へ知らせる
            // セットアップ・ハンドシェイク(詳細は呼び出し側のコメント参照)
            reached_read: Sender<()>,
        }
        impl std::io::Read for WedgedReader {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                // read() に到達した合図を送る。テスト側はこれを受け取ってから
                // 子プロセスの exit を発火させる(詳細は呼び出し側のコメント参照)
                let _ = self.reached_read.send(());
                // 対応する Sender は通常 drop されない設計なので、この recv() は
                // 通常決して返らない(孫プロセスに slave を握られ続けている状態を
                // 模す)。ただし、このテストが(タイミング flake 等で)途中で
                // panic してスタック巻き戻り時に Sender が drop された場合でも
                // `unreachable!()` で二次 panic を起こしてしまうと、本来の
                // 失敗理由がこのスレッドの panic メッセージに埋もれてしまう。
                // それを避けるため、recv() が Err で返っても park() で
                // 眠り直すだけにする(ポーリングではなく、誰にも起こされない
                // ため実質的に無期限にブロックし続ける = production コードが
                // 前提とする「read() が永久に戻らない」を安全に維持する)
                loop {
                    let _ = self.block.recv();
                    std::thread::park();
                }
            }
        }

        let (tx, rx) = channel::<Ev>();
        let sink: Arc<dyn PtySink> = Arc::new(ChannelSink { tx });
        let backpressure = Arc::new(Backpressure::new());
        let alive = Arc::new(AtomicBool::new(true));

        // 対応する Sender を意図的に保持したまま drop しない(このテスト関数の
        // スコープを抜けるとプロセスごと破棄される)。reader スレッドは
        // `read()` から永久に戻らず、production コードの設計どおりリークする
        let (_never_signaled, block_rx) = channel::<()>();
        // セットアップ・ハンドシェイク用チャネル。これが無いと、waiter の
        // close()(`backpressure.rs::wait_until_drained()` は `closed` を
        // `pending` より先に判定する)が reader の最初の
        // `wait_until_drained()` より先に届くことがあり、reader が一度も
        // `read()` を呼ばずに break してしまう(= wedge が成立しないまま
        // on_exit が数百マイクロ秒で届き、下の `elapsed >= 300ms` が偶発的に
        // red になるセットアップ・レース)。reader が `read()` に到達した
        // ことを確認してから子プロセスの exit を発火させることで、この
        // レースを決定的に排除する
        let (reached_read_tx, reached_read_rx) = channel::<()>();
        let reader = WedgedReader {
            block: block_rx,
            reached_read: reached_read_tx,
        };
        let (drain_tx, drain_rx) = channel::<()>();
        let reader_handle = spawn_reader_thread(
            "surf".to_string(),
            Box::new(reader),
            Arc::clone(&backpressure),
            Arc::clone(&sink),
            drain_tx,
        )
        .expect("spawn reader thread");

        // reader が read() に到達するまで待つ(上記ハンドシェイクの受信側)。
        // 上限を切り、タイムアウトしたら「reader が read() に到達しなかった」
        // と分かるメッセージで明示的に panic させる
        reached_read_rx
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or_else(|err| {
                panic!(
                    "reader が read() に到達しなかった(セットアップに失敗した \
                 疑いがある): {err}"
                )
            });

        let (exit_tx, exit_rx) = channel::<()>();
        exit_tx.send(()).expect("pre-fire fake process exit");
        let fake_child = FakeChild {
            exit_gate: Mutex::new(exit_rx),
        };
        let pid = Arc::new(Mutex::new(None));
        let started = std::time::Instant::now();
        spawn_waiter_thread(
            "surf".to_string(),
            Box::new(fake_child),
            reader_handle,
            Arc::clone(&backpressure),
            Arc::clone(&alive),
            pid,
            sink,
            drain_rx,
        )
        .expect("spawn waiter thread");

        // このテスト自身の待ちは必ず上限を切る(production 内部の deadline
        // 値そのものに結合させないよう、意図的に余裕を持たせた 3 秒とする)。
        // 超えたら明示的に panic させる
        let event = rx
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or_else(|err| {
                panic!(
                    "on_exit が 3 秒以内に届かなかった: reader が read() で wedge した \
                 ときに waiter が join() を無条件に待ってハングしている疑いがある ({err})"
                )
            });
        let elapsed = started.elapsed();
        assert!(
            matches!(event, Ev::Exit(Some(0))),
            "actual event: {event:?}"
        );
        assert!(!alive.load(Ordering::SeqCst));
        // reader が wedge しているため、waiter は内部の join deadline まで
        // 律儀に待ってから on_exit を出しているはず。ほぼ即座に返ってしまうと、
        // 待たずに on_exit を出す(=バグを別の形で踏んでいる)か、たまたま
        // wedge を再現できていない疑いがある
        assert!(
            elapsed >= Duration::from_millis(300),
            "on_exit が早く届きすぎた ({elapsed:?}): join deadline を待たずに \
             on_exit を出している、または reader の wedge を再現できていない疑いがある"
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
        // 通常経路(reader が `PTY_JOIN_DEADLINE` 内に終了する)では、
        // `spawn_waiter_thread` が `reader_handle.join()` の完了を待ってから
        // `sink.on_exit()` を呼ぶため、Exit は reader が終了するまで送られない
        // (この経路は
        // `waiter_wakes_a_reader_parked_on_high_water_and_still_delivers_on_exit_promptly`
        // が運用している)。ただし Timeout 分岐(孫プロセスが slave を握って reader が
        // `read()` で wedge した場合)ではこの順序は成立せず、`pty://exit` の後に
        // `pty://data` が最大 1 件飛びうる(詳細は `PTY_JOIN_DEADLINE` のドキュメント
        // コメント参照)。このテストは通常経路のみを対象とするため、park から
        // 再開できたかどうかがこのテストの唯一の弁別点になる。
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
        let (drain_tx, drain_rx) = channel::<()>();
        let reader_handle = spawn_reader_thread(
            "surf".to_string(),
            Box::new(reader),
            Arc::clone(&backpressure),
            Arc::clone(&sink),
            drain_tx,
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
        let pid = Arc::new(Mutex::new(None));
        spawn_waiter_thread(
            "surf".to_string(),
            Box::new(fake_child),
            reader_handle,
            Arc::clone(&backpressure),
            Arc::clone(&alive),
            pid,
            sink,
            drain_rx,
        )
        .expect("spawn waiter thread");

        // `on_data` が実際に渡してきた seq で ack する。この時点ではまだ子プロセスの
        // 終了を発火していない(exit を先に送ると、waiter が close() を呼ぶタイミング
        // と reader が cv から目覚めて再開するタイミングとの間にレースが生まれ、
        // ack が正しく効いていても Exit が先に届いてしまう場合があるため)。
        backpressure.ack(last_seq);
        // ここは意図的に Backpressure::new() の既定 stall_timeout(5秒。
        // BACKPRESSURE_STALL_TIMEOUT)より短いタイムアウトにする。もし ack が
        // 効いていなくても、stall_timeout の安全弁が5秒後に会計を諦めて reader を
        // 再開させてしまい、seq 配線の変異(seq を定数0にする等)を検出できなく
        // なる(実測: 5秒のタイムアウトだと変異時も安全弁の再開で green になって
        // しまった)。2秒なら安全弁より確実に先にタイムアウトし、ack 由来の
        // 再開だけを弁別できる。
        let event = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("ack 後に何も届かなかった: on_data に渡す seq の配線が壊れている疑いがある");
        assert!(
            matches!(event, Ev::Data { .. }),
            "ack 後に追加の Data が届かなかった: seq の配線が壊れている疑いがある \
             (actual event: {event:?})"
        );

        // ここまでで seq の配線は弁別済み。後片付けとして子プロセスの終了を発火し、
        // Exit まで読み飛ばしてスレッドを回収する
        exit_tx.send(()).expect("signal fake process exit");
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
        //
        // fix round 1: `pwd -P` の後ろに `sleep 30` を残し、期待するパスが
        // 届いた時点で観測を打ち切って kill() する形に変更(理由は
        // observe_while_alive のドキュメントコメント参照)。
        let (tx, rx) = channel();
        let dir = tempfile::tempdir().expect("create temp dir");
        let mut s = spec("/bin/sh", &["-c", "pwd -P; sleep 30"]);
        s.cwd = dir.path().to_path_buf();
        let surface = PtySurface::spawn(s, Arc::new(ChannelSink { tx })).expect("spawn /bin/sh");
        let expected = std::fs::canonicalize(dir.path())
            .expect("canonicalize temp dir")
            .to_string_lossy()
            .into_owned();
        let out = observe_while_alive(&rx, &surface, Duration::from_secs(10), |acc| {
            acc.trim() == expected
        });
        assert_eq!(out.trim(), expected, "actual output: {out:?}");
        surface.kill().expect("kill");
        drain_until_exit_after_kill(&rx, &surface);
    }

    #[test]
    fn spawn_creates_the_pty_with_the_given_cols_and_rows() {
        // Important B の弁別: `PtySize { cols, rows, .. }` の配線を定数
        // (DEFAULT_COLS/DEFAULT_ROWS = 80x24) に変異させる変異が生存しないことを
        // 確認するテスト。既定値と明確に異なる cols/rows を渡し、子プロセスの
        // `stty size`(macOS では "rows cols" の順で出力)がその値を報告する
        // ことを確認する。
        //
        // fix round 1: `stty size` の後ろに `sleep 30` を残せるよう `/bin/sh
        // -c` 経由で実行し、期待する値が届いた時点で観測を打ち切って
        // kill() する形に変更(理由は observe_while_alive のドキュメント
        // コメント参照)。
        let (tx, rx) = channel();
        let mut s = spec("/bin/sh", &["-c", "stty size; sleep 30"]);
        s.cols = 100;
        s.rows = 40;
        assert_ne!(s.cols, DEFAULT_COLS);
        assert_ne!(s.rows, DEFAULT_ROWS);
        let surface = PtySurface::spawn(s, Arc::new(ChannelSink { tx })).expect("spawn /bin/sh");
        let out = observe_while_alive(&rx, &surface, Duration::from_secs(10), |acc| {
            acc.trim() == "40 100"
        });
        assert_eq!(out.trim(), "40 100", "actual output: {out:?}");
        surface.kill().expect("kill");
        drain_until_exit_after_kill(&rx, &surface);
    }

    #[test]
    fn spawn_passes_the_given_env_to_the_child_process() {
        // Important B の弁別: `SpawnSpec.env` を子プロセスへ渡す `cmd.env(...)`
        // ループを削除する変異が生存しないことを確認するテスト。契約 §15 は
        // 消費者を名指ししている(M1-4 がここに `("KAMUX_SESSION_ID",
        // session.id)` を入れる)ため、env の弁別テストは不可欠。
        //
        // cwd/cols_rows のテストと同じ作法(observe_while_alive /
        // drain_until_exit_after_kill)に倣い、子プロセスを生かしたまま出力を
        // 観測してから kill() する(理由は observe_while_alive のドキュメント
        // コメント参照)。
        let (tx, rx) = channel();
        let mut s = spec(
            "/bin/sh",
            &["-c", "printf \"%s\\n\" \"$KAMUX_TEST_ENV\"; sleep 30"],
        );
        s.env = vec![("KAMUX_TEST_ENV".to_string(), "kamux-env-ok".to_string())];
        let surface = PtySurface::spawn(s, Arc::new(ChannelSink { tx })).expect("spawn /bin/sh");
        let out = observe_while_alive(&rx, &surface, Duration::from_secs(10), |acc| {
            acc.contains("kamux-env-ok")
        });
        assert!(out.contains("kamux-env-ok"), "actual output: {out:?}");
        surface.kill().expect("kill");
        drain_until_exit_after_kill(&rx, &surface);
    }

    #[test]
    fn write_reaches_the_child_process() {
        // fix round 2(指摘1、案A採用): 新しい openpty はデフォルトで line
        // discipline の ECHO が有効なため、`cat` が実際に fd を読んでいなくても、
        // 書き込んだバイト列は PTY 層のエコーとして master 側に戻ってくる。
        // fix round 1 のテストは Ctrl-D 相当の終了待ちを経由しないため、
        // 「write が PTY master に届く」ことしか証明できておらず、テスト名が
        // 主張する「write が子プロセスに届く」ことは証明できていなかった
        // (`write_all` を握り潰す変異も ECHO 経由の応答と `cat` の出力の両方を
        // 同時に消すため判別不能)。
        //
        // 子側で `stty -echo` してからエコーを止め、`cat` の出力のみで判定する
        // (指摘の (A) 案。応答文字列が2回現れることをアサートする (B) 案より、
        // 意図が明確でエコーが混ざらないため採用)。`stty -echo` の完了と
        // `write()` の間にレースが生まれないよう、`stty -echo` の直後に子側が
        // 出す目印(`kamux-ready`)を読み取ってから `write()` する
        // (`kill_terminates_a_process_that_ignores_sighup` と同じレース対策)。
        //
        // `stty -echo` 自体が(制御端末が無い等の理由で)静かに失敗すると、
        // このテストは「ECHO 経由の応答」を「cat の出力」と誤認したまま
        // green になりかねない(実測: 実 PTY 経由では `stty -echo` は
        // `rc=0` で成功し `stty -a` が `-echo` を報告することを確認済みだが、
        // 環境差でこの前提が崩れた場合に静かに壊れるのは避けたい)。
        // そのため `stty -echo` の成否を子プロセス自身に報告させ、失敗した
        // 場合はテストを明示的に落とす(`kamux-stty-failed` 目印)。
        let (tx, rx) = channel();
        let surface = PtySurface::spawn(
            spec(
                "/bin/sh",
                &[
                    "-c",
                    "if stty -echo; then printf '%s\\n' kamux-ready; \
                     else printf '%s\\n' kamux-stty-failed; fi; cat",
                ],
            ),
            Arc::new(ChannelSink { tx }),
        )
        .expect("spawn /bin/sh");
        let mut ready = String::new();
        loop {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(Ev::Data { base64, seq }) => {
                    ready.push_str(&String::from_utf8_lossy(
                        &BASE64.decode(base64).expect("valid base64"),
                    ));
                    surface.ack(seq);
                    if ready.contains("kamux-stty-failed") {
                        panic!(
                            "stty -echo が失敗した: ECHO が有効なままの可能性があり、\
                             この後の判定が cat の出力かどうか保証できない \
                             (actual output so far: {ready:?})"
                        );
                    }
                    if ready.contains("kamux-ready") {
                        break;
                    }
                }
                other => panic!("readiness marker が届く前に想定外のイベント: {other:?}"),
            }
        }
        surface.write(b"ping-kamux\n").expect("write line");
        let out = observe_while_alive(&rx, &surface, Duration::from_secs(10), |acc| {
            acc.contains("ping-kamux")
        });
        assert!(out.contains("ping-kamux"), "actual output: {out:?}");
        surface.kill().expect("kill");
        drain_until_exit_after_kill(&rx, &surface);
    }

    #[test]
    fn resize_updates_the_pty_window_size() {
        let (tx, rx) = channel();
        let surface = PtySurface::spawn(spec("/bin/cat", &[]), Arc::new(ChannelSink { tx }))
            .expect("spawn /bin/cat");
        assert_eq!(
            surface.size().expect("initial size"),
            (DEFAULT_COLS, DEFAULT_ROWS)
        );
        surface.resize(120, 40).expect("resize");
        assert_eq!(surface.size().expect("resized size"), (120, 40));
        surface.kill().expect("kill");
        let _ = drain(&rx, &surface);
    }

    #[test]
    fn resize_clamps_zero_to_one() {
        let (tx, rx) = channel();
        let surface = PtySurface::spawn(spec("/bin/cat", &[]), Arc::new(ChannelSink { tx }))
            .expect("spawn /bin/cat");
        surface.resize(0, 0).expect("resize");
        assert_eq!(surface.size().expect("size"), (1, 1));
        surface.kill().expect("kill");
        let _ = drain(&rx, &surface);
    }

    #[test]
    fn kill_terminates_a_long_running_child_and_emits_exit() {
        let (tx, rx) = channel();
        let surface = PtySurface::spawn(spec("/bin/cat", &[]), Arc::new(ChannelSink { tx }))
            .expect("spawn /bin/cat");
        assert!(surface.is_alive());
        surface.kill().expect("kill");
        // Exit イベントが届く(drain は Exit で戻る)
        let _ = drain(&rx, &surface);
        assert!(!surface.is_alive());
    }

    #[test]
    fn kill_is_idempotent() {
        let (tx, rx) = channel();
        let surface = PtySurface::spawn(spec("/bin/cat", &[]), Arc::new(ChannelSink { tx }))
            .expect("spawn /bin/cat");
        surface.kill().expect("first kill");
        let _ = drain(&rx, &surface);
        // 契約 §15: 既に死んでいても Ok を返す
        surface.kill().expect("second kill must be Ok");
        surface.kill().expect("third kill must be Ok");
    }

    #[test]
    fn kill_terminates_a_process_that_ignores_sighup() {
        // Important(b) の弁別: portable-pty の `ChildKiller::kill`
        // (`clone_killer()` が返す `ProcessSignaller`)は unix では SIGHUP を
        // 1発送るだけでエスカレーションが無い。SIGHUP をハンドルして無視する
        // プロセスに対して SIGHUP だけを送る実装だと、このプロセスは永久に
        // 生き続け、この後の `drain` がタイムアウトして red になる。
        // 契約 §15 は「SIGKILL 相当」を要求しており、SIGKILL はハンドラで
        // 無視できないため、この弁別で kill() の強さを固定する。
        let (tx, rx) = channel();
        let surface = PtySurface::spawn(
            spec(
                "/bin/sh",
                &[
                    "-c",
                    "trap '' HUP; echo kamux-ready; while true; do sleep 1; done",
                ],
            ),
            Arc::new(ChannelSink { tx }),
        )
        .expect("spawn /bin/sh");
        assert!(surface.is_alive());
        // `trap` の設置が完了する前に SIGHUP を送ると、まだ既定動作(Term)の
        // ままの子プロセスがたまたま死んでしまい、この弁別が偽陽性で green に
        // なってしまう(実測済み: 目印を待たずに送ると SIGHUP 単発の変異でも
        // green になった)。子プロセスが trap 設置後に出す目印を読み取って
        // からのみ kill() を送ることで、このレースを排除する。
        let mut out = String::new();
        loop {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(Ev::Data { base64, seq }) => {
                    out.push_str(&String::from_utf8_lossy(
                        &BASE64.decode(base64).expect("valid base64"),
                    ));
                    surface.ack(seq);
                    if out.contains("kamux-ready") {
                        break;
                    }
                }
                other => panic!("readiness marker が届く前に想定外のイベント: {other:?}"),
            }
        }
        surface.kill().expect("kill");
        let _ = drain(&rx, &surface);
        assert!(!surface.is_alive());
    }

    #[test]
    fn kill_after_reap_does_not_signal_a_recycled_pid() {
        // Important 1 の弁別: waiter は `child.wait()` で reap した直後、
        // `pid` mutex を保持したまま `None` を書き込む。on_exit を受け取った
        // 時点で(waiter スレッド内で on_exit 送出は None 書き込みの後に行われる
        // ため)、`surface.pid` は必ず `None` になっているはずである。もし
        // waiter の「pid を None にする」処理が抜けていると、ここは red になる。
        // その状態で `kill()` を呼んでも、裸の pid へシグナルが飛ばないこと
        // (= Ok(()) を返すこと)を確認する。
        let (tx, rx) = channel();
        let surface = PtySurface::spawn(
            spec("/bin/echo", &["reap-then-kill"]),
            Arc::new(ChannelSink { tx }),
        )
        .expect("spawn /bin/echo");
        let (_out, code) = drain(&rx, &surface);
        assert_eq!(code, Some(0));
        assert!(
            surface
                .pid
                .lock()
                .expect("pid mutex not poisoned")
                .is_none(),
            "reap 後も pid が Some のまま残っている(TOCTOU の窓が塞がっていない疑い)"
        );
        assert!(surface.kill().is_ok(), "reap 後の kill() は Ok を返すべき");
    }

    #[test]
    fn dropping_a_surface_without_kill_still_terminates_the_child_process() {
        // Critical(a) の弁別: `PtySurface` の `master`/`writer` フィールドを
        // 単純にフィールド単位で drop するだけでは、SIGHUP をハンドル/無視する
        // よう構成された子プロセス(`trap '' HUP`)を終了させられない。
        //
        // 弁別対象を /bin/cat のような SIGHUP デフォルト動作(終了)のプロセス
        // にすると、たとえ Drop を実装し忘れていても、master/writer が閉じる
        // ことで発生する PTY 側の hang up 相当のイベントに乗って「たまたま」
        // 死んでしまい、この弁別テストが偽陽性で green になる
        // (実測済み: Drop を消してもこのケースは 11ms 以内に exit code 0 で
        // 終了した)。SIGHUP を無視するプロセスに対してのみ、Drop の有無が
        // 観測可能な差になる(実測: Drop を消すと 5 秒待っても生存し続けた)。
        //
        // ここでは kill() を一切呼ばずに `PtySurface` を drop し、OS レベルで
        // (signal 0 による存在確認)子プロセスが実際に消えたことを確認する。
        let (tx, rx) = channel();
        let surface = PtySurface::spawn(
            spec(
                "/bin/sh",
                &[
                    "-c",
                    "trap '' HUP; echo kamux-ready; while true; do sleep 1; done",
                ],
            ),
            Arc::new(ChannelSink { tx }),
        )
        .expect("spawn /bin/sh");
        let pid = surface
            .pid
            .lock()
            .expect("pid mutex not poisoned")
            .expect("pid captured at spawn");
        // trap 設置前に何かが起きて偶然死ぬ余地を無くすため、trap 設置後に
        // 子プロセスが出す目印が届くまで待ってから drop する
        let mut out = String::new();
        loop {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(Ev::Data { base64, seq }) => {
                    out.push_str(&String::from_utf8_lossy(
                        &BASE64.decode(base64).expect("valid base64"),
                    ));
                    surface.ack(seq);
                    if out.contains("kamux-ready") {
                        break;
                    }
                }
                other => panic!("readiness marker が届く前に想定外のイベント: {other:?}"),
            }
        }
        drop(surface);
        // PtySurface は drop 済みだが、reader/waiter スレッドは自分が持つ
        // Arc<dyn PtySink> クローンで動き続けるため、on_exit は引き続き
        // rx から届く。ポーリングせず、イベント到着を待つだけで済む
        loop {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ev::Exit(_)) => break,
                Ok(Ev::Data { .. }) => continue,
                Err(err) => panic!(
                    "Drop 後に on_exit が届かなかった: master fd がリークして \
                     子プロセスが終了していない疑いがある ({err})"
                ),
            }
        }
        // イベントに加え、OS レベルでも実際にプロセスが消えたことを直接確認する
        // (signal 0: 存在確認のみでシグナルは送らない)
        let alive_at_os_level = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
        assert!(
            !alive_at_os_level,
            "on_exit は届いたが OS 上にはまだ pid={pid} が残っている"
        );
    }

    #[test]
    fn reader_pauses_at_high_water_and_resumes_after_ack() {
        // Task 2〜4 の結線を実 PTY(合成 Read/Child ではなく本物の /usr/bin/yes)
        // で検証する。子プロセスは生かしたまま観測を終える形にすることで、
        // 「バックプレッシャーで停止中に子プロセスが終了すると macOS の
        // ttywait/ttyclose(最後の slave fd が閉じる際、カーネルが master 側の
        // drain を最大 ~600ms 待ってから未読分を破棄する)により出力末尾が
        // 失われうる」既知の受容済み欠陥を踏まない(lane-controller 裁定で
        // parked = 受容。このテストでは踏む必要がない)。
        use crate::pty::backpressure::BACKPRESSURE_HIGH_WATER;

        let (tx, rx) = channel();
        // /usr/bin/yes は無限に出力し続ける。PATH に依存しないよう絶対パスで指定する
        let surface = PtySurface::spawn(
            spec("/usr/bin/yes", &["kamux"]),
            Arc::new(ChannelSink { tx }),
        )
        .expect("spawn /usr/bin/yes");

        // ack をせずに高水位まで受け取る
        let mut last_seq = 0u64;
        while surface.pending_bytes() < BACKPRESSURE_HIGH_WATER {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(Ev::Data { seq, .. }) => last_seq = seq,
                other => panic!("expected pty data before high water, got {other:?}"),
            }
        }

        // 送出済みで未受信のチャンクを、来なくなるまで吸い出す(sleep による
        // 同期ではなく、短いタイムアウトが Err(Timeout) を返した時点を
        // 「これ以上チャンクが来ない」の決定的な合図として扱う。負荷下の
        // --test-threads=2 でも reader がまだ送出中のチャンクを取りこぼして
        // 後段の assert が偽陽性で red になることを避ける)。
        // reader がゲートで止まらない退行(バックプレッシャーが効かず
        // /usr/bin/yes の出力が来続ける)が起きた場合、このループは
        // recv_timeout(300ms) を毎回リセットしながら際限なく回り続けて
        // ハングしうる(実測済み: mutation 確認で 132.8 秒ハングし、外部から
        // 子プロセスを kill するまで終わらなかった)。「必ず recv_timeout 等で
        // 上限を切り、タイムアウトしたら明示的に panic する」という要件を
        // 満たすため、ドレイン開始からの総経過時間にも別枠で上限を設け、
        // 超えたら明示的に panic させる(正しい実装は最後のチャンクから
        // 300ms 以内にここを抜けるため、5 秒は十分すぎる余裕)
        let drain_started = std::time::Instant::now();
        loop {
            match rx.recv_timeout(Duration::from_millis(300)) {
                Ok(Ev::Data { seq, .. }) => last_seq = seq,
                Ok(other) => panic!("expected pty data while draining, got {other:?}"),
                Err(_) => break,
            }
            assert!(
                drain_started.elapsed() < Duration::from_secs(5),
                "reader never stopped draining within 5s: backpressure gate may be gone \
                 (reader not pausing at high water)"
            );
        }
        assert!(surface.pending_bytes() >= BACKPRESSURE_HIGH_WATER);

        // ack すると読み取りが再開する
        surface.ack(last_seq);
        assert!(
            matches!(rx.recv_timeout(Duration::from_secs(5)), Ok(Ev::Data { .. })),
            "reader must resume after ack"
        );

        surface.kill().expect("kill");
        // 子プロセスの後始末が非同期に走るため、Exit を受け取るまで待って
        // からテストを終える(残留プロセス/スレッドが後続テストへ漏れるのを防ぐ)
        loop {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ev::Data { .. }) => continue,
                Ok(Ev::Exit(_)) => break,
                Err(err) => panic!("exit event never arrived after kill: {err}"),
            }
        }
    }

    #[test]
    fn killing_a_paused_surface_still_emits_exit() {
        // 実測でわかった注記(重要): このテストは元々「kill() 冒頭の
        // `self.backpressure.close()` を消すと 2 秒の閾値を超えて red になる」
        // という弁別を狙っていたが、`spawn_waiter_thread` は `child.wait()`
        // 復帰直後に `backpressure.close()` を無条件に呼ぶため、kill() 側の
        // close() の有無に関わらず「時間内に Exit が届く」結果は変わらない
        // (実測: どちらの実装でも一貫して ~602ms。macOS の ttywait が
        // child.wait() 自体の復帰を ~600ms 遅らせるのが支配的要因で、これは
        // kill() の close() の有無とは無関係)。close() 単体の弁別は Task 4 で
        // 既に `FakeChild`(child.wait() の復帰タイミングを完全に制御できる
        // 合成子プロセス)を使った
        // `waiter_wakes_a_reader_parked_on_high_water_and_still_delivers_on_exit_promptly`
        // が担っており、そちらは実測でこの弁別に成功している。
        //
        // そのためこのテストは「時間」ではなく、
        // 「バックプレッシャーで停止中の surface を kill しても、
        // デッドロックせず Exit が届き is_alive() が false になる」という
        // 実 PTY 上の結合的事実だけを検証する(close() 行それ自体の弁別では
        // ない)。タイムアウトは実測フロア(~600ms)+ 負荷時の余裕を見て 5 秒とする。
        use crate::pty::backpressure::BACKPRESSURE_HIGH_WATER;

        let (tx, rx) = channel();
        let surface = PtySurface::spawn(
            spec("/usr/bin/yes", &["kamux"]),
            Arc::new(ChannelSink { tx }),
        )
        .expect("spawn /usr/bin/yes");

        while surface.pending_bytes() < BACKPRESSURE_HIGH_WATER {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(Ev::Data { .. }) => {}
                other => panic!("expected pty data before high water, got {other:?}"),
            }
        }

        surface.kill().expect("kill");
        loop {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ev::Data { .. }) => continue,
                Ok(Ev::Exit(_)) => break,
                Err(err) => panic!(
                    "exit event never arrived: backpressure による停止と kill() が \
                     デッドロックしている疑いがある ({err})"
                ),
            }
        }
        assert!(!surface.is_alive());
    }

    // --- 契約 §15 (kill は完全に冪等) の EPERM 対応の観測 -----------------
    // 詳細な機構は契約 §98.1 の表(行1〜6)を参照。
    // ここでは「なぜ EPERM も冪等扱いか」を純関数の単体テストとして
    // 固定する。`fork` + `setsid` を使って OS の挙動そのものを固定する回帰
    // テストは契約 §98.11 が却下した(守る対象が挙動ではなく理由になる。かつ
    // マルチスレッドのテストプロセスで `fork` するハザードを買う)。

    #[test]
    fn is_nothing_left_to_signal_is_true_for_esrch() {
        assert!(is_nothing_left_to_signal(libc::ESRCH));
    }

    #[test]
    fn is_nothing_left_to_signal_is_true_for_eperm() {
        assert!(is_nothing_left_to_signal(libc::EPERM));
    }

    #[test]
    fn is_nothing_left_to_signal_is_false_for_einval() {
        assert!(!is_nothing_left_to_signal(libc::EINVAL));
    }
}
