//! セッション 1 個分の出力活動の観測と、沈黙の検出。
//!
//! ホットパス（`record_output`）はロックを一切取らず Atomic だけを触る。
//! 沈黙ウォッチャはセッションごと最大 1 本で、発火したら自ら終了する。
//! アイドル時にタスクが 1 本も存在しないため、契約 §0「アイドル CPU ほぼ 0%」を満たす。
//!
//! `silence_timeout_ms` はここでは丸めない。許容範囲へのクランプは
//! `registry::clamp_timeout_secs`（Task 9）の責務であり、`SessionActivity` は
//! 渡された値をそのまま使う。
//!
//! # `watcher_alive` の解除と再チェックに `SeqCst` を使う理由（緩めてはならない）
//!
//! ウォッチャは「フラグを降ろす → もう一度状態を読む」、`record_output` /
//! `rearm_after` は「状態を書く → フラグを取りに行く」という順で動く。
//! 2 者がそれぞれ「自分の store の後に、相手が書く変数を load する」形は
//! **store-buffering** であり、release/acquire では**両方が相手の旧値を見る**結果を
//! 禁止できない —— `store` には synchronizes-with が生えないし、RMW の
//! modification order もこの 2 組の前後関係を決めないからである。
//! 禁止するには 4 アクセスすべてが同じ全順序に載る必要があり、それが `SeqCst` である
//! （`store` と `load` の間に両者で `fence(SeqCst)` を置いても同じ結果になる）。
//! **本ターゲットの aarch64（Apple Silicon）は StoreLoad の並べ替えを架構上許すため、
//! 実機で起こりうる。**
//!
//! 対になるのは次の 2 組で、どちらも 4 アクセス全部が `SeqCst` でなければ閉じない。
//! **片側だけ強めても閉じない。**
//!
//! 1. `watcher_alive.store(false)` → `last_activity_ms.load`（`watch_silence` の Fire 腕）
//!    ↔ `last_activity_ms.store` → `watcher_alive.swap(true)`（`record_output`）
//! 2. `watcher_alive.store(false)` → `enabled.load`（`watch_silence` の disabled 腕の再チェック）
//!    ↔ `enabled.store`（`reconfigure`）→ `watcher_alive.swap(true)`（`record_output` / `rearm_after`）
//!
//! 緩めたときの帰結は「**ウォッチャが消え、そのアイドル期間の `Silence` が永久に出ない**」。
//! この harness（単一スレッド・仮想時間の `#[tokio::test(start_paused = true)]`）では
//! メモリ順序を観測できず、観測するには `loom` のようなモデル検査が要る（本タスクの scope 外）。
//! **したがってこの注記が唯一の防護である。「過剰だから」と `Relaxed` / `AcqRel` へ戻さないこと。**
//!
//! ホットパス（`record_output`）が余分に払うのは 1 チャンクあたり `SeqCst` の store 1 本
//! （aarch64 では `stlr` 1 命令）と、元から RMW だった `swap` の格上げだけである。
//! 同じチャンクに対して既に行っている base64 エンコード（契約 §8）より桁で小さい
//! （設計 §4.1 と同じ論法）。

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc::UnboundedSender, Notify};

use super::clock::Clock;
use super::silence::{silence_step, SilenceStep};
use super::{HeuristicEvent, BEL_DEBOUNCE_MS};

pub struct SessionActivity {
    session_id: String,
    clock: Arc<dyn Clock>,
    tx: UnboundedSender<HeuristicEvent>,
    /// tokio ランタイム外の PTY 読み取りスレッドから spawn するために保持する。
    /// `Handle::current()` を内部で呼ばず、必ず呼び出し側から渡させる
    /// （構築位置がランタイム文脈にあるとは限らないため）
    rt: tokio::runtime::Handle,

    /// 最終出力活動時刻（epoch ms）
    last_activity_ms: AtomicI64,
    /// 沈黙タイムアウト（ms）。`reconfigure` でライブ変更される
    silence_timeout_ms: AtomicU64,
    /// セッション単位のオン/オフ
    enabled: AtomicBool,
    /// ウォッチャが 1 本だけ走ることを保証する
    watcher_alive: AtomicBool,
    /// BEL のデバウンス用。最後に報告した時刻
    last_bel_report_ms: AtomicI64,
    /// 設定変更で寝ているウォッチャを叩き起こす
    reconfigured: Notify,
}

impl SessionActivity {
    /// `rt` はウォッチャを spawn するランタイムハンドル。
    /// 本番は `tauri::async_runtime::handle().inner().clone()`、
    /// テストは `#[tokio::test(start_paused = true)]` 内の `Handle::current()` を渡す。
    /// ここで `Handle::current()` を呼ばないのが要点（構築位置がランタイム外でも panic しない）。
    pub fn new(
        session_id: String,
        clock: Arc<dyn Clock>,
        tx: UnboundedSender<HeuristicEvent>,
        rt: tokio::runtime::Handle,
        enabled: bool,
        silence_timeout_ms: u64,
    ) -> Arc<Self> {
        let now = clock.now_ms();
        Arc::new(Self {
            session_id,
            clock,
            tx,
            rt,
            last_activity_ms: AtomicI64::new(now),
            silence_timeout_ms: AtomicU64::new(silence_timeout_ms),
            enabled: AtomicBool::new(enabled),
            watcher_alive: AtomicBool::new(false),
            last_bel_report_ms: AtomicI64::new(i64::MIN / 2),
            reconfigured: Notify::new(),
        })
    }

    /// PTY 読み取りスレッドから毎チャンク呼ばれるホットパス。ロックを取らない。
    ///
    /// `SeqCst` が 2 箇所ある理由はモジュール doc の「`watcher_alive` の解除と再チェック」を参照。
    /// 代償は 1 チャンクあたりバリア 1 本で、同じチャンクの base64 エンコードより桁で小さい。
    pub fn record_output(self: &Arc<Self>, bel_count: usize) {
        // この load はモジュール doc の 2 つの対のどちらにも含まれない
        // （フラグを降ろす側と取る側の交差ではない）。ホットパスなので `Relaxed` で足りる。
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }

        let now = self.clock.now_ms();
        // 対 1 の W2。この直後の `watcher_alive.swap`（R2）との組が、
        // Fire 腕の store/load の組と交差する
        self.last_activity_ms.store(now, Ordering::SeqCst);

        if bel_count > 0 && now - self.last_bel_report_ms.load(Ordering::Relaxed) >= BEL_DEBOUNCE_MS
        {
            self.last_bel_report_ms.store(now, Ordering::Relaxed);
            let _ = self.tx.send(HeuristicEvent::Bel {
                session_id: self.session_id.clone(),
            });
        }

        // ウォッチャが既にいれば何もしない。タイマーを作り直さないのが要点。
        // 対 1・対 2 の R2（`SeqCst` の理由はモジュール doc）
        if !self.watcher_alive.swap(true, Ordering::SeqCst) {
            self.spawn_watcher(0);
        }
    }

    /// 出力が無くてもウォッチャを立て直す。
    ///
    /// 受け手は **Task 9（`HeuristicRegistry`）の消費ループ**である ——
    /// ゲート規則 3（`hook_liveness == Pending`）で沈黙イベントを抑止したとき、
    /// 猶予切れの後にもう一度評価させるために呼ぶ。呼ばなければ、
    /// `silence_timeout_secs` が猶予（`HOOK_GRACE_MS`）より短い claude セッションで
    /// 沈黙推定が二度と発火しない。
    ///
    /// `delay_ms` は**必ず呼び出し側が計算する**。`SessionActivity` は `cli_kind` も
    /// `spawned_at` も知らないため、ここで猶予を計算してはならない。
    ///
    /// ウォッチャが既に生きていれば何もしない。`last_activity_ms` も書き換えない
    /// （出力が無かった時刻を活動時刻として記録してはならない）。無効なセッションで
    /// 黙るのは `watch_silence` 先頭の `enabled` 判定が担う。
    pub fn rearm_after(self: &Arc<Self>, delay_ms: u64) {
        // 対 2 の R2（`record_output` と同じ役目。`SeqCst` の理由はモジュール doc）
        if !self.watcher_alive.swap(true, Ordering::SeqCst) {
            self.spawn_watcher(delay_ms);
        }
    }

    /// セッション設定のライブ変更。寝ているウォッチャを起こして再評価させる。
    pub fn reconfigure(&self, enabled: bool, silence_timeout_ms: u64) {
        // 対 2 の W2。disabled 腕の再チェック（`enabled.load`）との組が
        // `watcher_alive` の store/swap の組と交差する（モジュール doc 参照）
        self.enabled.store(enabled, Ordering::SeqCst);
        self.silence_timeout_ms
            .store(silence_timeout_ms, Ordering::Relaxed);
        self.reconfigured.notify_waiters();
    }

    /// `watcher_alive` を立てた側だけが呼ぶ。`initial_delay_ms` だけ待ってから監視に入る。
    fn spawn_watcher(self: &Arc<Self>, initial_delay_ms: u64) {
        let me = Arc::clone(self);
        self.rt.spawn(async move {
            if initial_delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(initial_delay_ms)).await;
            }
            me.watch_silence().await;
        });
    }

    async fn watch_silence(self: Arc<Self>) {
        loop {
            // ループ先頭の判定は `Relaxed` で足りる —— 順序が要るのは
            // 「フラグを降ろした後」の再チェック側であり、こちらではない
            // （同一変数の後続 load はコヒーレンスによりこれより古い値を読まない）
            if !self.enabled.load(Ordering::Relaxed) {
                // 対 2 の W1（`SeqCst` の理由はモジュール doc）
                self.watcher_alive.store(false, Ordering::SeqCst);

                // 対 2 の R1: フラグを降ろした後に再有効化を見直す。
                // ここが無いと、`reconfigure(true, …)` が上の load と store の間に
                // 割り込んだとき、同時刻の `record_output` は `watcher_alive == true` を
                // 見て spawn せず、ウォッチャだけが消える。
                // 順序が要点: `enabled` の load が先、`swap` が後。逆に書くと
                // 無効なまま `watcher_alive` を立てて return し、以後 `record_output` も
                // `rearm_after` も永久に spawn しなくなる（短絡評価に依存している）。
                //
                // 閉じるのは「`reconfigure(true, …)` がフラグを降ろす前に着弾していた」窓だけである。
                // 無効な間に有効化され、その後 1 チャンクも出力が来ない場合の再 arm は
                // ここでは扱わない（消費側 = Task 9 の責務）。
                if self.enabled.load(Ordering::SeqCst)
                    && !self.watcher_alive.swap(true, Ordering::SeqCst)
                {
                    continue;
                }
                return;
            }

            let timeout_ms = self.silence_timeout_ms.load(Ordering::Relaxed);
            // 待ち時間の基準にするだけの読みなので `Relaxed`。対 1 に含まれるのは
            // 再チェック（下の `SeqCst` の load）であってこちらではない。
            // ⚠️ ただし古い値（= より小さい基準）を読むと経過時間が過大になり、
            // `Silence` が 1 本余分に出る余地はある。ウォッチャ自体は再チェックが
            // 新しい値を見てループを続けるので消えない。`4a353cf` 以前からある性質で、
            // 消すならこの load も `SeqCst` にする必要がある
            let last = self.last_activity_ms.load(Ordering::Relaxed);

            match silence_step(self.clock.now_ms(), last, timeout_ms) {
                SilenceStep::Wait { ms } => {
                    // 設定変更が来たら満了を待たずに再評価する（ポーリングにはならない）
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(ms)) => {}
                        _ = self.reconfigured.notified() => {}
                    }
                }
                SilenceStep::Fire => {
                    let _ = self.tx.send(HeuristicEvent::Silence {
                        session_id: self.session_id.clone(),
                    });
                    // 対 1 の W1（`SeqCst` の理由はモジュール doc）
                    self.watcher_alive.store(false, Ordering::SeqCst);

                    // 対 1 の R1: store の直前に届いた出力を取りこぼさない（lost wakeup 対策）。
                    // この 4 アクセス（ここの store/load と `record_output` の store/swap）が
                    // すべて `SeqCst` でなければ、「こちらが旧 `last_activity_ms` を読み、
                    // かつ `record_output` の swap が `true` を返す」結果を禁止できず、
                    // ウォッチャが消えてこのアイドル期間の `Silence` が永久に出ない。
                    // 順序は disabled 腕と同じく load が先・swap が後（短絡評価）。
                    if self.last_activity_ms.load(Ordering::SeqCst) > last
                        && !self.watcher_alive.swap(true, Ordering::SeqCst)
                    {
                        continue;
                    }
                    return;
                }
            }
        }
    }
}

impl std::fmt::Debug for SessionActivity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionActivity")
            .field("session_id", &self.session_id)
            .field("enabled", &self.enabled.load(Ordering::Relaxed))
            .field(
                "silence_timeout_ms",
                &self.silence_timeout_ms.load(Ordering::Relaxed),
            )
            .field("watcher_alive", &self.watcher_alive.load(Ordering::Relaxed))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::heuristics::clock::TestClock;
    use std::time::Duration;
    use tokio::sync::mpsc;

    /// 仮想時計と tokio の仮想時間を同時に進める。実時間は 1 ミリ秒も進まない。
    async fn advance(clock: &TestClock, ms: u64) {
        clock.advance_ms(ms as i64);
        tokio::time::advance(Duration::from_millis(ms)).await;
        tokio::task::yield_now().await;
    }

    fn setup(
        timeout_ms: u64,
    ) -> (
        TestClock,
        Arc<SessionActivity>,
        mpsc::UnboundedReceiver<HeuristicEvent>,
    ) {
        let clock = TestClock::new(0);
        let (tx, rx) = mpsc::unbounded_channel();
        let act = SessionActivity::new(
            "s1".to_string(),
            Arc::new(clock.clone()),
            tx,
            tokio::runtime::Handle::current(),
            true,
            timeout_ms,
        );
        (clock, act, rx)
    }

    #[tokio::test(start_paused = true)]
    async fn fires_silence_after_the_timeout() {
        let (clock, act, mut rx) = setup(30_000);
        act.record_output(0);
        tokio::task::yield_now().await; // ウォッチャに最初の sleep を登録させる

        advance(&clock, 29_000).await;
        assert!(rx.try_recv().is_err(), "29 秒では発火してはいけない");

        advance(&clock, 1_500).await;
        assert_eq!(
            rx.try_recv().unwrap(),
            HeuristicEvent::Silence {
                session_id: "s1".into()
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn continued_output_postpones_the_firing() {
        let (clock, act, mut rx) = setup(30_000);
        act.record_output(0);
        tokio::task::yield_now().await;

        for _ in 0..5 {
            advance(&clock, 20_000).await;
            act.record_output(0); // 20 秒ごとに出力 → 一度も沈黙しない
        }
        assert!(rx.try_recv().is_err(), "出力が続く限り発火してはいけない");

        advance(&clock, 31_000).await;
        assert_eq!(
            rx.try_recv().unwrap(),
            HeuristicEvent::Silence {
                session_id: "s1".into()
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn fires_only_once_per_silence_period() {
        let (clock, act, mut rx) = setup(30_000);
        act.record_output(0);
        tokio::task::yield_now().await;

        advance(&clock, 31_000).await;
        assert!(rx.try_recv().is_ok());

        // 発火後はウォッチャが終了しているので、放置しても二度と発火しない
        advance(&clock, 300_000).await;
        assert!(rx.try_recv().is_err(), "沈黙 1 回につき 1 イベントのはず");
    }

    #[tokio::test(start_paused = true)]
    async fn output_after_firing_restarts_the_watcher() {
        let (clock, act, mut rx) = setup(30_000);
        act.record_output(0);
        tokio::task::yield_now().await;
        advance(&clock, 31_000).await;
        assert!(rx.try_recv().is_ok());

        act.record_output(0);
        tokio::task::yield_now().await;
        advance(&clock, 31_000).await;
        assert_eq!(
            rx.try_recv().unwrap(),
            HeuristicEvent::Silence {
                session_id: "s1".into()
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn bel_is_reported_immediately() {
        let (_clock, act, mut rx) = setup(30_000);
        act.record_output(2);
        assert_eq!(
            rx.try_recv().unwrap(),
            HeuristicEvent::Bel {
                session_id: "s1".into()
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn zero_bel_count_reports_nothing() {
        let (_clock, act, mut rx) = setup(30_000);
        act.record_output(0);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn consecutive_bels_are_debounced() {
        let (clock, act, mut rx) = setup(30_000);
        act.record_output(1);
        assert!(rx.try_recv().is_ok());

        clock.advance_ms(BEL_DEBOUNCE_MS - 1);
        act.record_output(1);
        assert!(rx.try_recv().is_err(), "デバウンス窓の中は 1 件に丸める");

        clock.advance_ms(2);
        act.record_output(1);
        assert!(rx.try_recv().is_ok(), "窓を越えたら再び報告する");
    }

    #[tokio::test(start_paused = true)]
    async fn disabled_activity_reports_nothing() {
        let clock = TestClock::new(0);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let act = SessionActivity::new(
            "s1".to_string(),
            Arc::new(clock.clone()),
            tx,
            tokio::runtime::Handle::current(),
            false,
            30_000,
        );

        act.record_output(3);
        tokio::task::yield_now().await;
        advance(&clock, 60_000).await;
        assert!(rx.try_recv().is_err(), "無効なセッションは一切報告しない");
    }

    #[tokio::test(start_paused = true)]
    async fn reconfigure_shortens_the_wait_of_a_sleeping_watcher() {
        // 設定変更でウォッチャを叩き起こして再評価させる（設計 §4.3）
        let (clock, act, mut rx) = setup(300_000);
        act.record_output(0);
        tokio::task::yield_now().await;

        advance(&clock, 10_000).await;
        assert!(rx.try_recv().is_err());

        act.reconfigure(true, 5_000); // 5 秒へ短縮 → 経過 10 秒なので即発火するはず
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        assert_eq!(
            rx.try_recv().unwrap(),
            HeuristicEvent::Silence {
                session_id: "s1".into()
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn reconfigure_to_disabled_stops_reporting() {
        let (clock, act, mut rx) = setup(30_000);
        act.record_output(0);
        tokio::task::yield_now().await;

        act.reconfigure(false, 30_000);
        advance(&clock, 60_000).await;
        assert!(rx.try_recv().is_err());
    }

    /// 無効化でウォッチャが降りた後に再有効化し、**次の出力チャンク**が来たら立ち直る。
    /// disabled 腕が `watcher_alive` を確実に降ろしていることの観測点である ——
    /// 特に再チェックの短絡順序を取り違えて（`swap` を `enabled` の load より先に書いて）
    /// フラグを立てたまま return すると、以後 `record_output` も `rearm_after` も
    /// 二度と spawn しなくなる。既存の disabled 系テストは「発火しないこと」しか見ないため、
    /// その取り違えを 1 本も捕まえられない。
    ///
    /// ⚠️ この経路は disabled 腕の**再チェックそのもの**は通らない（再チェックが効く
    /// interleaving は単一スレッド・仮想時間の harness では作れない。モジュール doc 参照）。
    /// 出力が来ないまま再有効化された場合の再 arm は消費側（Task 9）の責務である。
    #[tokio::test(start_paused = true)]
    async fn output_after_re_enabling_restarts_the_watcher() {
        let (clock, act, mut rx) = setup(30_000);
        act.record_output(0);
        tokio::task::yield_now().await;

        act.reconfigure(false, 30_000);
        advance(&clock, 60_000).await; // ウォッチャは disabled 腕で降りる
        assert!(rx.try_recv().is_err(), "無効な間は発火しない");

        act.reconfigure(true, 30_000);
        act.record_output(0); // 再有効化後の最初のチャンク
        tokio::task::yield_now().await;

        advance(&clock, 31_000).await;
        assert_eq!(
            rx.try_recv().unwrap(),
            HeuristicEvent::Silence {
                session_id: "s1".into()
            },
            "再有効化後の出力でウォッチャが立ち直っていない（watcher_alive が立ったままの疑い）"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn five_sessions_each_fire_exactly_once() {
        // セッション数に比例してタイマーが増えることの確認（設計 §4.3）
        let clock = TestClock::new(0);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let acts: Vec<_> = (0..5)
            .map(|i| {
                SessionActivity::new(
                    format!("s{i}"),
                    Arc::new(clock.clone()),
                    tx.clone(),
                    tokio::runtime::Handle::current(),
                    true,
                    30_000,
                )
            })
            .collect();
        for a in &acts {
            a.record_output(0);
        }
        tokio::task::yield_now().await;

        advance(&clock, 31_000).await;
        let mut ids = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            ids.push(ev.session_id().to_string());
        }
        ids.sort();
        assert_eq!(ids, vec!["s0", "s1", "s2", "s3", "s4"]);
    }

    /// 出力が無くてもウォッチャを立て直せる（Task 9 のゲート抑止からの再評価）。
    /// 途中の否定 assert が「遅延を無視して即発火する」実装を落とす。
    #[tokio::test(start_paused = true)]
    async fn rearm_after_fires_again_without_new_output() {
        let (clock, act, mut rx) = setup(5_000);
        act.record_output(0);
        tokio::task::yield_now().await;

        advance(&clock, 5_500).await;
        assert_eq!(
            rx.try_recv().unwrap(),
            HeuristicEvent::Silence {
                session_id: "s1".into()
            }
        );

        act.rearm_after(15_000);
        tokio::task::yield_now().await;

        advance(&clock, 14_000).await;
        assert!(
            rx.try_recv().is_err(),
            "指定した遅延が明けるまで発火してはいけない"
        );

        advance(&clock, 1_100).await;
        assert_eq!(
            rx.try_recv().unwrap(),
            HeuristicEvent::Silence {
                session_id: "s1".into()
            },
            "遅延が明けたら出力が無くても沈黙を再評価する"
        );
    }

    /// `rearm_after` は活動時刻を書き換えない —— 出力が無かった時刻を活動として
    /// 記録すると、沈黙の基準がずれて発火が遅れる。
    /// `delay_ms < timeout_ms` のときにだけ両者は区別できる（Task 9 が猶予の残りだけを
    /// 渡すと実際にこの関係になる）。
    #[tokio::test(start_paused = true)]
    async fn rearm_after_keeps_the_original_activity_timestamp() {
        let (clock, act, mut rx) = setup(5_000);
        act.record_output(0);
        tokio::task::yield_now().await;

        advance(&clock, 5_500).await;
        assert_eq!(
            rx.try_recv().unwrap(),
            HeuristicEvent::Silence {
                session_id: "s1".into()
            }
        );

        // 遅延（1 秒）はタイムアウト（5 秒）より短い。活動時刻を「今」に書き換える実装だと
        // 遅延明けの経過時間が 1.1 秒しかなくなり、沈黙が消えてしまう
        act.rearm_after(1_000);
        tokio::task::yield_now().await;

        advance(&clock, 1_100).await;
        assert_eq!(
            rx.try_recv().unwrap(),
            HeuristicEvent::Silence {
                session_id: "s1".into()
            },
            "沈黙の基準は最後に出力があった時刻のままでなければならない"
        );
    }

    /// `rearm_after` がセッション単位のオフスイッチの迂回路になってはならない。
    #[tokio::test(start_paused = true)]
    async fn rearm_after_on_a_disabled_activity_reports_nothing() {
        let clock = TestClock::new(0);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let act = SessionActivity::new(
            "s1".to_string(),
            Arc::new(clock.clone()),
            tx,
            tokio::runtime::Handle::current(),
            false,
            30_000,
        );

        act.rearm_after(1_000);
        tokio::task::yield_now().await;
        advance(&clock, 60_000).await;
        assert!(rx.try_recv().is_err(), "無効なセッションは rearm でも黙る");
    }

    /// ウォッチャが生きている間の `rearm_after` は二重ウォッチャを作らない。
    #[tokio::test(start_paused = true)]
    async fn rearm_after_is_a_noop_while_a_watcher_is_alive() {
        let (clock, act, mut rx) = setup(30_000);
        act.record_output(0);
        tokio::task::yield_now().await;

        act.rearm_after(1_000);
        tokio::task::yield_now().await;

        advance(&clock, 31_000).await;
        assert_eq!(
            rx.try_recv().unwrap(),
            HeuristicEvent::Silence {
                session_id: "s1".into()
            }
        );
        assert!(
            rx.try_recv().is_err(),
            "沈黙 1 回につきイベントは 1 本のはず（ウォッチャが二重に立っている）"
        );
    }

    /// `watch_silence` が `last` を読んだ後・`watcher_alive` を降ろす前に出力が届く競合を、
    /// 単一スレッドの仮想時間上で決定的に再現する時計。
    /// `fire_at_ms` 以降の最初の `now_ms()` 呼び出しの内側で `record_output` を 1 度だけ差し込む。
    struct RaceClock {
        inner: TestClock,
        fire_at_ms: i64,
        target: std::sync::OnceLock<std::sync::Weak<SessionActivity>>,
        armed: AtomicBool,
        hook_ran: AtomicBool,
    }

    impl RaceClock {
        fn new(inner: TestClock, fire_at_ms: i64) -> Self {
            Self {
                inner,
                fire_at_ms,
                target: std::sync::OnceLock::new(),
                armed: AtomicBool::new(true),
                hook_ran: AtomicBool::new(false),
            }
        }

        /// 差し込みが実際に走ったか。走っていなければこのテストは何も測っていない。
        fn hook_ran(&self) -> bool {
            self.hook_ran.load(Ordering::SeqCst)
        }
    }

    impl Clock for RaceClock {
        fn now_ms(&self) -> i64 {
            let now = self.inner.now_ms();
            // `armed` を先に降ろすので、差し込んだ `record_output` からの再入は素通りする
            if now >= self.fire_at_ms && self.armed.swap(false, Ordering::SeqCst) {
                if let Some(act) = self.target.get().and_then(std::sync::Weak::upgrade) {
                    act.record_output(0);
                    self.hook_ran.store(true, Ordering::SeqCst);
                }
            }
            now
        }
    }

    /// 発火の直前に届いた出力を取りこぼさない（lost wakeup 対策）。
    /// 1 本目の発火時刻は競合の帰結なので固定しない。荷重は 2 本目の受信にある。
    #[tokio::test(start_paused = true)]
    async fn output_arriving_as_the_watcher_fires_keeps_the_watcher_alive() {
        let base = TestClock::new(0);
        let clock = Arc::new(RaceClock::new(base.clone(), 31_000));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let act = SessionActivity::new(
            "s1".to_string(),
            clock.clone(),
            tx,
            tokio::runtime::Handle::current(),
            true,
            30_000,
        );
        let _ = clock.target.set(Arc::downgrade(&act));

        act.record_output(0);
        tokio::task::yield_now().await;

        advance(&base, 31_000).await;
        assert!(
            clock.hook_ran(),
            "競合の差し込みが起きていない —— このテストは何も測っていない"
        );
        let _ = rx.try_recv();

        // 出力が届いた以上、ウォッチャは生き残って次の沈黙を測り直さねばならない
        advance(&base, 31_000).await;
        assert_eq!(
            rx.try_recv().unwrap(),
            HeuristicEvent::Silence {
                session_id: "s1".into()
            },
            "発火と同時に届いた出力が取りこぼされ、ウォッチャが立ち消えている"
        );
    }
}
