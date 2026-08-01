// src-tauri/src/pty/backpressure.rs
use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// 1 MiB 未確認で読み取り停止(契約 §9)
pub const BACKPRESSURE_HIGH_WATER: usize = 1 << 20;
/// 256 KiB まで減ったら再開(契約 §9)
pub const BACKPRESSURE_LOW_WATER: usize = 1 << 18;
/// 1 回の read で受け取る最大バイト数(契約 §9)
pub const PTY_READ_CHUNK: usize = 8 * 1024;
/// ack が完全に途絶えたときにバックプレッシャーを諦めるまでの時間。
/// WebView のリロードで listener が消えると ack が二度と来ないため、
/// これが無いと reader も waiter も永久に止まり pty://exit が飛ばなくなる。
pub const BACKPRESSURE_STALL_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Default)]
struct Inner {
    /// 未 ack のチャンク (seq, byte_len)。seq 昇順
    inflight: VecDeque<(u64, usize)>,
    pending: usize,
    next_seq: u64,
    last_acked: u64,
    closed: bool,
}

/// PTY 読み取りスレッドの滞留バイト会計。
/// 高水位を超えている間は読み取りスレッドを Condvar で眠らせる(ポーリングしない)。
#[derive(Debug)]
pub struct Backpressure {
    inner: Mutex<Inner>,
    cv: Condvar,
    stall_timeout: Duration,
}

impl Default for Backpressure {
    fn default() -> Self {
        Self::new()
    }
}

impl Backpressure {
    pub fn new() -> Self {
        Self::with_stall_timeout(BACKPRESSURE_STALL_TIMEOUT)
    }

    pub fn with_stall_timeout(stall_timeout: Duration) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            cv: Condvar::new(),
            stall_timeout,
        }
    }

    /// 中毒したロックからも回復する(panic 経路を作らない)
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// チャンク送出を記録し、割り当てた seq を返す
    pub fn record(&self, len: usize) -> u64 {
        let mut inner = self.lock();
        inner.next_seq += 1;
        inner.pending += len;
        let seq = inner.next_seq;
        inner.inflight.push_back((seq, len));
        seq
    }

    /// seq までの消化を反映し、低水位まで戻っていれば読み取りスレッドを起こす。
    ///
    /// 不変条件: `seq` はこのインスタンスの `record` が発行したもの
    /// (`seq <= next_seq`) でなければならない。再 spawn 後の新しいサーフェスは
    /// `next_seq = 0` から始まるため、旧サーフェス由来の高い seq をここで受理すると
    /// `last_acked` が `next_seq` を超えて毒され、以降の正当な ack が
    /// `seq <= last_acked` で恒久的に捨てられる(clamp ではなく無視で防ぐ)。
    pub fn ack(&self, seq: u64) {
        let mut inner = self.lock();
        if seq > inner.next_seq {
            return;
        }
        if seq <= inner.last_acked {
            return;
        }
        inner.last_acked = seq;
        while let Some(&(front_seq, len)) = inner.inflight.front() {
            if front_seq > seq {
                break;
            }
            inner.pending = inner.pending.saturating_sub(len);
            inner.inflight.pop_front();
        }
        if inner.pending <= BACKPRESSURE_LOW_WATER {
            self.cv.notify_all();
        }
    }

    pub fn pending(&self) -> usize {
        self.lock().pending
    }

    pub fn is_paused(&self) -> bool {
        self.lock().pending >= BACKPRESSURE_HIGH_WATER
    }

    pub fn is_closed(&self) -> bool {
        self.lock().closed
    }

    /// 読み取り再開まで待つ。`false` を返したら読み取りスレッドは終了する
    pub fn wait_until_drained(&self) -> bool {
        let mut inner = self.lock();
        if inner.closed {
            return false;
        }
        if inner.pending < BACKPRESSURE_HIGH_WATER {
            return true;
        }
        let mut deadline = Instant::now() + self.stall_timeout;
        let mut last_pending = inner.pending;
        while inner.pending > BACKPRESSURE_LOW_WATER && !inner.closed {
            // 直前の周回より pending が減っていれば「ack は生きている」とみなし
            // stall deadline を延長する。比較対象は毎周この値で更新する
            // (関数入口の値と比較すると一度減っただけで安全弁が永久に無効化される)
            if inner.pending < last_pending {
                deadline = Instant::now() + self.stall_timeout;
            }
            last_pending = inner.pending;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                // ack が完全に途絶えた。会計を捨てて読み取りを再開する(契約 §9 の安全弁)
                inner.inflight.clear();
                inner.pending = 0;
                break;
            }
            let (next, _timeout) = self
                .cv
                .wait_timeout(inner, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            inner = next;
        }
        !inner.closed
    }

    /// サーフェス終了時に呼ぶ。停止中の読み取りスレッドを起こして終了させる
    pub fn close(&self) {
        let mut inner = self.lock();
        inner.closed = true;
        self.cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn record_assigns_monotonic_seq_and_accumulates_pending() {
        let bp = Backpressure::new();
        assert_eq!(bp.record(100), 1);
        assert_eq!(bp.record(200), 2);
        assert_eq!(bp.pending(), 300);
    }

    #[test]
    fn ack_drains_inflight_up_to_seq() {
        let bp = Backpressure::new();
        bp.record(100);
        bp.record(200);
        bp.record(400);
        bp.ack(2);
        assert_eq!(bp.pending(), 400);
        bp.ack(3);
        assert_eq!(bp.pending(), 0);
    }

    #[test]
    fn ack_ignores_stale_and_duplicate_seq() {
        // inflight が空でない状態で stale/duplicate ack を投げる。
        // front_seq > seq でのループ停止や pop の実装を壊すと 400/0 の期待値が崩れる。
        let bp = Backpressure::new();
        bp.record(100); // seq1
        bp.record(200); // seq2
        bp.record(400); // seq3
        bp.ack(2);
        assert_eq!(bp.pending(), 400);
        bp.ack(2); // duplicate
        assert_eq!(bp.pending(), 400);
        bp.ack(1); // stale (last_acked より小さい)
        assert_eq!(bp.pending(), 400);
        bp.ack(3);
        assert_eq!(bp.pending(), 0);
    }

    // 注記: 上のテストは `seq <= last_acked` ガードそのものは弁別しない。
    // `record` が seq を厳密に単調増加で発行し、pop ループが `front_seq > seq` で
    // 停止する限り、常に `front_seq > last_acked` が成り立つため
    // (ack が処理した時点で front はその ack の seq より必ず先へ進んでいる)、
    // ガードを消しても `pending()` は変化しない。ガードが実際に効くのは
    // 次の `ack_ignores_seq_from_a_different_backpressure_instance` が示す
    // 「`next_seq` を超える seq を受理してしまい `last_acked` が毒される」経路であり、
    // それは `seq > next_seq` チェック(Important 2)の役目である。

    #[test]
    fn ack_ignores_seq_from_a_different_backpressure_instance() {
        // 再 spawn 後の旧サーフェス由来の seq (このインスタンスの next_seq を超える)
        // を受理すると last_acked が next_seq を超えて毒され、正当な ack が
        // 以降すべて `seq <= last_acked` で捨てられる。ここでは即時の誤 drain も検証する。
        let bp = Backpressure::new();
        bp.record(100); // seq1
        bp.record(200); // seq2, next_seq = 2, pending = 300
        bp.ack(9_999); // 別インスタンス由来。next_seq を超えるので無視されるべき
        assert_eq!(bp.pending(), 300);
        bp.ack(2); // 正当な ack は引き続き処理される
        assert_eq!(bp.pending(), 0);
    }

    #[test]
    fn is_paused_only_at_or_above_high_water() {
        let bp = Backpressure::new();
        bp.record(BACKPRESSURE_HIGH_WATER - 1);
        assert!(!bp.is_paused());
        bp.record(1);
        assert!(bp.is_paused());
    }

    #[test]
    fn wait_until_drained_returns_immediately_below_high_water() {
        let bp = Backpressure::new();
        bp.record(BACKPRESSURE_LOW_WATER);
        let started = Instant::now();
        assert!(bp.wait_until_drained());
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn wait_until_drained_wakes_immediately_when_ack_reaches_low_water() {
        // notify_all 経路の弁別: stall_timeout をわざと長く取り、ack が来た直後に
        // 起床することを検証する。ack から notify_all を消すと、この上限を超えて
        // stall_timeout 満了まで眠り続ける。
        let stall_timeout = Duration::from_millis(500);
        let bp = Arc::new(Backpressure::with_stall_timeout(stall_timeout));
        let seq = bp.record(BACKPRESSURE_HIGH_WATER);
        let acker = Arc::clone(&bp);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            acker.ack(seq);
        });
        let started = Instant::now();
        assert!(bp.wait_until_drained());
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(50));
        assert!(elapsed < Duration::from_millis(300));
        assert_eq!(bp.pending(), 0);
    }

    #[test]
    fn wait_until_drained_resets_stall_deadline_on_progress_and_outlives_a_single_stall_timeout() {
        // 指摘1の弁別: 生きている遅い consumer。各 ack は stall_timeout より短い間隔で
        // 届くが、全体の所要時間は stall_timeout を優に超える。deadline を進捗のたびに
        // 延長しないと ~stall_timeout で強制 drain され、下限アサーションを割る。
        let stall_timeout = Duration::from_millis(250);
        let bp = Arc::new(Backpressure::with_stall_timeout(stall_timeout));
        let seq1 = bp.record(BACKPRESSURE_HIGH_WATER);
        let seq2 = bp.record(BACKPRESSURE_HIGH_WATER);
        let acker = Arc::clone(&bp);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            acker.ack(seq1); // 進捗はあるが低水位までは戻らない(notify されない)
            std::thread::sleep(Duration::from_millis(300));
            acker.ack(seq2); // 低水位まで戻る(notify される)
        });
        let started = Instant::now();
        assert!(bp.wait_until_drained());
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(350));
        assert_eq!(bp.pending(), 0);
    }

    #[test]
    fn close_unblocks_waiter_and_returns_false() {
        let bp = Arc::new(Backpressure::new());
        bp.record(BACKPRESSURE_HIGH_WATER);
        let closer = Arc::clone(&bp);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            closer.close();
        });
        let started = Instant::now();
        assert!(!bp.wait_until_drained());
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(40));
        // notify_all が無ければ default stall_timeout (5s) 満了まで待つことになり、
        // この上限を超える
        assert!(elapsed < Duration::from_millis(500));
    }

    #[test]
    fn stalled_waiter_gives_up_backpressure_after_stall_timeout() {
        // 指摘3の弁別: ack が完全に停止するケース。安全弁 (stall_timeout) が
        // 生きていることを検証する。
        let bp = Backpressure::with_stall_timeout(Duration::from_millis(120));
        bp.record(BACKPRESSURE_HIGH_WATER);
        let started = Instant::now();
        // ack が一切来なくても、タイムアウト後に会計を捨てて読み取りを再開する
        assert!(bp.wait_until_drained());
        assert!(started.elapsed() >= Duration::from_millis(110));
        assert_eq!(bp.pending(), 0);
    }
}
