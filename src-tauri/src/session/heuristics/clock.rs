//! 時刻の抽象。テストが実時間を待たずに済むよう `now_ms` だけを注入可能にする。
//!
//! `sleep` はこの trait に含めない。tokio の `start_paused = true` が
//! `tokio::time::sleep` を仮想化してくれるため、注入が必要なのは現在時刻だけ。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Unix epoch ミリ秒を返す時計。契約 §3 の時刻表現と揃える。
pub trait Clock: Send + Sync + 'static {
    fn now_ms(&self) -> i64;
}

/// 本番用。システム時計の epoch ミリ秒。
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// テスト用。`advance_ms` で明示的に進める。クローンは同じ時刻を共有する。
#[derive(Debug, Clone, Default)]
pub struct TestClock(Arc<AtomicI64>);

impl TestClock {
    pub fn new(start_ms: i64) -> Self {
        Self(Arc::new(AtomicI64::new(start_ms)))
    }

    pub fn advance_ms(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_clock_returns_epoch_millis() {
        let now = SystemClock.now_ms();
        // 2020-01-01T00:00:00Z より後、2100 年より前であること
        assert!(now > 1_577_836_800_000, "epoch ms として小さすぎる: {now}");
        assert!(now < 4_102_444_800_000, "epoch ms として大きすぎる: {now}");
    }

    #[test]
    fn system_clock_is_non_decreasing() {
        let a = SystemClock.now_ms();
        let b = SystemClock.now_ms();
        assert!(b >= a);
    }

    #[test]
    fn test_clock_starts_at_given_value_and_advances() {
        let clock = TestClock::new(1_000);
        assert_eq!(clock.now_ms(), 1_000);
        clock.advance_ms(30_000);
        assert_eq!(clock.now_ms(), 31_000);
        clock.advance_ms(1);
        assert_eq!(clock.now_ms(), 31_001);
    }

    #[test]
    fn test_clock_clones_share_the_same_time() {
        let clock = TestClock::new(0);
        let cloned = clock.clone();
        clock.advance_ms(500);
        assert_eq!(cloned.now_ms(), 500);
    }

    #[test]
    fn test_clock_is_usable_as_dyn_clock() {
        let clock = TestClock::new(42);
        let dynamic: std::sync::Arc<dyn Clock> = std::sync::Arc::new(clock.clone());
        assert_eq!(dynamic.now_ms(), 42);
        clock.advance_ms(8);
        assert_eq!(dynamic.now_ms(), 50);
    }
}
