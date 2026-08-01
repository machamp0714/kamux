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
/// 停止中に Condvar を起こし直す間隔。停止していないときは 1 度も使われない
const STALL_WAKE_STEP: Duration = Duration::from_secs(1);

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
    wake_step: Duration,
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
            wake_step: stall_timeout.min(STALL_WAKE_STEP),
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

    /// seq までの消化を反映し、低水位まで戻っていれば読み取りスレッドを起こす
    pub fn ack(&self, seq: u64) {
        let mut inner = self.lock();
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
        let deadline = Instant::now() + self.stall_timeout;
        while inner.pending > BACKPRESSURE_LOW_WATER && !inner.closed {
            if Instant::now() >= deadline {
                // フロントが ack を返さなくなった。会計を捨てて読み取りを再開する
                inner.inflight.clear();
                inner.pending = 0;
                break;
            }
            let (next, _timeout) = self
                .cv
                .wait_timeout(inner, self.wake_step)
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
        let bp = Backpressure::new();
        bp.record(100);
        bp.record(200);
        bp.ack(2);
        bp.ack(2);
        bp.ack(1);
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
    fn wait_until_drained_blocks_until_ack_reaches_low_water() {
        let bp = Arc::new(Backpressure::new());
        let seq = bp.record(BACKPRESSURE_HIGH_WATER);
        let acker = Arc::clone(&bp);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            acker.ack(seq);
        });
        let started = Instant::now();
        assert!(bp.wait_until_drained());
        assert!(started.elapsed() >= Duration::from_millis(70));
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
        assert!(!bp.wait_until_drained());
    }

    #[test]
    fn stalled_waiter_gives_up_backpressure_after_stall_timeout() {
        let bp = Backpressure::with_stall_timeout(Duration::from_millis(120));
        bp.record(BACKPRESSURE_HIGH_WATER);
        let started = Instant::now();
        // ack が一切来なくても、タイムアウト後に会計を捨てて読み取りを再開する
        assert!(bp.wait_until_drained());
        assert!(started.elapsed() >= Duration::from_millis(110));
        assert_eq!(bp.pending(), 0);
    }
}
