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
//! ⚠️ ここで「観測できない」と言っているのは**メモリ順序だけ**である。
//! 「フラグを降ろした後に再チェックする」こと自体は interleaving の性質であって
//! 順序の性質ではないので、割り込み点さえ作れば観測できる —— disabled 腕には
//! `#[cfg(test)]` の seam（`tests::run_disabled_arm_seam`）を置いて決定的に観測している。
//! 2 つを混同して「再チェックの観測にも `loom` が要る」と読まないこと。
//!
//! # Fire 腕で `watcher_alive.store(false)` を `tx.send` より**前**に置く理由（入れ替えてはならない）
//!
//! 消費側（`HeuristicRegistry`）は `Silence` を受け取ってから `rearm_after` を呼びうる ——
//! ゲート規則 3（`hook_liveness == Pending`）で抑止したときに、猶予切れの後で
//! もう一度評価させるためである。`rearm_after` は `watcher_alive.swap(true)` が `true` を
//! 返したら**何もしない**ので、その呼び出しが `store(false)` より前に着弾すると空振りし、
//! 直後にウォッチャ自身も return する。帰結は `SeqCst` を緩めたときと同じ
//! 「**ウォッチャが消え、そのアイドル期間の `Silence` が永久に出ない**」である。
//!
//! `store(false)` を先に置くと、この窓は**メモリモデルの上で**閉じる ——
//! `store` は同一スレッドの `tx.send` に happens-before し、`send` は受信側の `recv` に
//! happens-before する。したがって消費側が `Silence` を見た時点で `watcher_alive == false` は
//! 可視であり、`swap` は必ず `false` を返して新しいウォッチャを立てる。
//! **上の `SeqCst` の議論と違ってアーキテクチャ依存ではない**（ここで効いているのは
//! store-buffering の禁止ではなく、チャンネルが張る happens-before である）。
//!
//! 順序を守らせる型もコンパイラ検査も無いので、観測点は
//! `tests::a_rearm_from_the_consumer_is_not_swallowed_by_the_firing_watcher` 1 本である
//! （`tx.send` 直後の `#[cfg(test)]` seam を使う）。**実測: 順序を戻すとこのテストだけが赤くなる。**
//!
//! フラグが降りている窓が `tx.send` の分だけ広がるが、その間に `record_output` が来ても
//! 新しいウォッチャが 1 本立つだけである（`swap` が相互排他を担う）。
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
        // ⚠️ この load は ISO のメモリモデル上はモジュール doc の**対 2 の輪の一部**である
        //（`SeqCst` へ格上げすれば対 2 が閉じる）。`Relaxed` のままで正しい根拠は
        // ISO のモデルではなく**アーキテクチャ側**にある: すぐ下の
        // `last_activity_ms.store`（対 1 の W2）が `SeqCst` で、aarch64 ではこれが
        // release store（`stlr`）へ落ちるため、先行するこの load を順序づける。
        // その結果、この load は下の `watcher_alive.swap`（対 1・対 2 の R2）より後ろへ
        // 動けず、輪は架構が閉じる。**つまり正しさはターゲット依存である。**
        // **依存**: その `SeqCst` store を弱める / 条件付きにする / この load と
        // `watcher_alive.swap` の間から外す、のいずれかを行うとこの根拠は静かに消える。
        // そのときはこの load 自身を `SeqCst` へ格上げすること
        //（ホットパスの代償がモジュール doc 末尾の記述より `ldar` 1 本ぶん増える点も直す）。
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

                // テスト専用の割り込み点（seam）。フラグを降ろした「後」・下の再チェックの
                // 「前」に、テストから `reconfigure(true, …)` を着弾させるために在る。
                // **これが無いと、下の再チェックのブロックを丸ごと消しても全テストが緑のまま**
                // になる（実測: round m3-3-t7-rerev-r1 の変異 B = 595 passed / 0 failed）。
                // 再チェックの存在は interleaving の性質なので、割り込み点さえ作れば
                // 単一スレッド・仮想時間のまま決定的に観測できる（`loom` が要るのは
                // メモリ順序のほうだけである）。`#[cfg(test)]` なので production には
                // 分岐も遅延もアトミック操作も 1 つも増えない。消す前に `tests` 側の
                // `run_disabled_arm_seam` の doc を読むこと。
                #[cfg(test)]
                tests::run_disabled_arm_seam();

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
                    // 対 1 の W1（`SeqCst` の理由はモジュール doc）。
                    // 🔴 **`tx.send` より前でなければならない**（モジュール doc
                    // 「消費側の再 arm を飲み込まない」を参照）。逆にすると、
                    // イベントを見た消費側の `rearm_after` がここより先に走り、
                    // `swap` が `true` を返して空振りする。
                    self.watcher_alive.store(false, Ordering::SeqCst);

                    let _ = self.tx.send(HeuristicEvent::Silence {
                        session_id: self.session_id.clone(),
                    });

                    // テスト専用の割り込み点（seam）。**消費側が動ける最初の瞬間**がここである
                    // （`tx.send` の後）。消費側は `Silence` を見てから `rearm_after` を
                    // 呼びうるので、その着弾を単一スレッド・仮想時間のまま決定的に作る。
                    #[cfg(test)]
                    tests::run_fire_arm_seam();

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
    use std::cell::RefCell;
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

    type SeamSlot = RefCell<Option<Box<dyn FnOnce()>>>;

    thread_local! {
        /// disabled 腕の割り込み点（seam）へ差し込む処理。1 回だけ走る（`take` する）。
        /// production 側の呼び出しは `#[cfg(test)]` なので、リリースビルドには 1 命令も残らない。
        static DISABLED_ARM_SEAM: SeamSlot = const { RefCell::new(None) };
        /// Fire 腕の `tx.send` 直後の割り込み点。消費側（`HeuristicRegistry`）が
        /// `Silence` を見て `rearm_after` を呼ぶ瞬間を、ここへ差し込んで再現する。
        static FIRE_ARM_SEAM: SeamSlot = const { RefCell::new(None) };
    }

    /// production の `watch_silence`（disabled 腕）から呼ばれる seam の本体。
    ///
    /// **何のために在るか**: 「`watcher_alive` を降ろした後の再チェック」は
    /// interleaving の性質なので、割り込み点さえあれば単一スレッド・仮想時間のまま
    /// 決定的に観測できる（メモリ順序と違って `loom` は要らない）。
    /// **この seam が無いと、再チェックのブロックを丸ごと消しても全テストが緑のままになる**
    /// —— 実測済み（round m3-3-t7-rerev-r1 の変異 B = 595 passed / 0 failed）。
    /// 差し込みが登録されていなければ何もしないので、他のテストの挙動は 1 つも変わらない。
    pub(super) fn run_disabled_arm_seam() {
        run_seam(&DISABLED_ARM_SEAM);
    }

    /// production の `watch_silence`（Fire 腕・`tx.send` の直後）から呼ばれる seam の本体。
    ///
    /// **何のために在るか**: 消費側は `Silence` を受け取ってから `rearm_after` を呼ぶ。
    /// その呼び出しが `watcher_alive.store(false)` より**前**に着弾すると `swap` が
    /// `true` を返して空振りし、直後にウォッチャも消える —— ウォッチャ 0 本・再 arm 無しで、
    /// その沈黙期間は二度と評価されない。順序（store が先、send が後）がこの窓を閉じており、
    /// **この seam がその順序の唯一の観測点である。**
    pub(super) fn run_fire_arm_seam() {
        run_seam(&FIRE_ARM_SEAM);
    }

    fn run_seam(slot: &'static std::thread::LocalKey<SeamSlot>) {
        // `f()` は借用の外で呼ぶ（差し込み先から再入しても `RefCell` を二重借用しない）
        let hook = slot.with(|s| s.borrow_mut().take());
        if let Some(f) = hook {
            f();
        }
    }

    /// 差し込みを登録する。戻り値の guard が落ちたら必ず外れる ——
    /// `--test-threads=1` では複数のテストが同じスレッドを共有するため、
    /// 走らずに残った差し込みが次のテストへ漏れるのを防ぐ（変異 C ではまさに走らない）。
    #[must_use]
    fn install_disabled_arm_seam(f: impl FnOnce() + 'static) -> SeamGuard {
        install_seam(&DISABLED_ARM_SEAM, f)
    }

    /// Fire 腕の seam 版。guard の役割は `install_disabled_arm_seam` と同じ。
    #[must_use]
    fn install_fire_arm_seam(f: impl FnOnce() + 'static) -> SeamGuard {
        install_seam(&FIRE_ARM_SEAM, f)
    }

    fn install_seam(
        slot: &'static std::thread::LocalKey<SeamSlot>,
        f: impl FnOnce() + 'static,
    ) -> SeamGuard {
        slot.with(|s| *s.borrow_mut() = Some(Box::new(f)));
        SeamGuard(slot)
    }

    /// `install_disabled_arm_seam` が返す RAII ガード。
    ///
    /// **何を防いでいるか**: libtest はワーカースレッドを複数テストへ使い回しうる。
    /// `DISABLED_ARM_SEAM` は `thread_local` なので、あるテストが差し込みを登録した後
    /// 消費されずに終わると（例: 途中で `panic` する、あるいは登録だけして走らせない書き方をする）、
    /// その状態が同じスレッドを再利用する次のテストへ残ってしまう可能性がある。
    /// `Drop` はテスト終了時にこの thread_local を確実に空へ落とし、その漏洩を防ぐ。
    ///
    /// **🔴 この防護には現状観測が無い（実測値つき）**: `Drop` の中身を no-op に変える変異は、
    /// 既定の並列実行（597 passed / 0 failed）でも `--test-threads=1`（81 passed / 0 failed）でも
    /// 全緑だった（2026-08-12、再レビュー round `m3-3-t7-rerev-r2` で実測）。
    /// 現行の 2 本のテストはどちらも `assert!(hook_ran)` で seam の消費を保証しており、
    /// 成功パスでは `take()` によって `Drop` を待たずに `DISABLED_ARM_SEAM` が空になるため、
    /// 今のテスト集合ではこの層が無くても結果は変わらない。
    /// **つまりこの層は「今のテスト集合では踏まない危険」に対する保険であり、
    /// 消しても現状のテストは何も言わない。**
    ///
    /// **なぜ漏洩を再現するテストを足さないか**: 漏洩を観測するには「登録だけして消費されずに
    /// 終わるテスト」を作る必要があるが、その振る舞いは libtest がワーカースレッドを使い回すという
    /// harness の実装詳細に依存する。これは安定した契約ではないため、依存したテストは
    /// harness の更新で意味を失うか、逆にフレークになる。
    ///
    /// 自分が登録した slot だけを空にする（登録先を保持する理由）。両方を空にする形にすると、
    /// 片方の guard が落ちたときにもう片方の差し込みまで消える。
    struct SeamGuard(&'static std::thread::LocalKey<SeamSlot>);

    impl Drop for SeamGuard {
        fn drop(&mut self) {
            self.0.with(|s| *s.borrow_mut() = None);
        }
    }

    /// disabled 腕の**再チェックそのもの**を観測するための段取り。
    ///
    /// `watcher_alive` を降ろした直後（= 再チェックが `enabled` を読む前）に
    /// `reconfigure(true, …)` が着弾する interleaving を、seam を使って決定的に作る。
    /// 着弾点は production コメントが宣言した射程の内側であり、
    /// deferred 項目（ウォッチャが終了しきった後の再有効化）ではない。
    ///
    /// 戻り値の `SeamGuard` は呼び出し側が保持すること（落とすと差し込みが外れる）。
    /// seam が実際に走ったことはここで assert する —— 走っていなければ、
    /// これを使うテストは再チェックを 1 つも測っていない（群 P）。
    async fn arrange_a_re_enable_landing_at_the_seam() -> (
        TestClock,
        Arc<SessionActivity>,
        mpsc::UnboundedReceiver<HeuristicEvent>,
        SeamGuard,
    ) {
        let (clock, act, rx) = setup(30_000);
        act.record_output(0);
        tokio::task::yield_now().await;

        let hook_ran = Arc::new(AtomicBool::new(false));
        let seam = {
            let act = Arc::clone(&act);
            let hook_ran = Arc::clone(&hook_ran);
            install_disabled_arm_seam(move || {
                act.reconfigure(true, 30_000);
                hook_ran.store(true, Ordering::SeqCst);
            })
        };

        // 寝ているウォッチャを起こして disabled 腕へ入れる。
        // その中で seam が走り、フラグ解除と再チェックの間に再有効化が着弾する
        act.reconfigure(false, 30_000);
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        assert!(
            hook_ran.load(Ordering::SeqCst),
            "seam が走っていない —— このテストは再チェックを何も測っていない"
        );

        (clock, act, rx, seam)
    }

    /// 再チェックが**在る**ことの観測点。無ければウォッチャが消え、
    /// このアイドル期間の `Silence` が永久に出ない。
    ///
    /// ⚠️ このテストで `rearm_after` を呼んではならない。再チェックを削除した実装でも
    /// `rearm_after` が 2 本目のウォッチャを立てて `Silence` を出してしまい、
    /// 観測が消える（実測: 呼んでいた版では再チェック削除の変異が緑になった）。
    /// フラグの取り直しは下の別テストで測る。
    #[tokio::test(start_paused = true)]
    async fn re_enabling_between_the_flag_clear_and_the_recheck_keeps_the_watcher_alive() {
        let (clock, _act, mut rx, _seam) = arrange_a_re_enable_landing_at_the_seam().await;

        advance(&clock, 31_000).await;
        assert_eq!(
            rx.try_recv().unwrap(),
            HeuristicEvent::Silence {
                session_id: "s1".into()
            },
            "フラグ解除と再チェックの間に着弾した再有効化を取りこぼし、ウォッチャが消えている"
        );
    }

    /// 再チェックが `watcher_alive` を**取り直している**ことの観測点。
    /// `swap` を `load` に書き換えるとフラグが空いたままになり、
    /// 続く `rearm_after` が 2 本目のウォッチャを立てて `Silence` が 2 本出る。
    #[tokio::test(start_paused = true)]
    async fn the_recheck_reclaims_the_watcher_flag() {
        let (clock, act, mut rx, _seam) = arrange_a_re_enable_landing_at_the_seam().await;

        // 再チェックがフラグを取り直していれば、これは no-op になる
        act.rearm_after(0);
        tokio::task::yield_now().await;

        advance(&clock, 31_000).await;
        assert!(rx.try_recv().is_ok(), "沈黙が 1 本も出ていない");
        assert!(
            rx.try_recv().is_err(),
            "ウォッチャが 2 本立っている —— 再チェックが watcher_alive を取り直していない"
        );
    }

    /// 無効化でウォッチャが降りた後に再有効化し、**次の出力チャンク**が来たら立ち直る。
    /// disabled 腕が `watcher_alive` を確実に降ろしていることの観測点である ——
    /// 特に再チェックの短絡順序を取り違えて（`swap` を `enabled` の load より先に書いて）
    /// フラグを立てたまま return すると、以後 `record_output` も `rearm_after` も
    /// 二度と spawn しなくなる。既存の disabled 系テストは「発火しないこと」しか見ないため、
    /// その取り違えを 1 本も捕まえられない。
    ///
    /// ⚠️ この経路は disabled 腕の**再チェックそのもの**は通らない（ここでの再有効化は
    /// ウォッチャが降りきった後に着弾するため）。再チェックが効く interleaving は
    /// `#[cfg(test)]` の seam を使って別の 2 本
    /// （`re_enabling_between_the_flag_clear_and_the_recheck_keeps_the_watcher_alive` /
    /// `the_recheck_reclaims_the_watcher_flag`）で決定的に作ってある ——
    /// **「この harness では作れない」は偽である。**
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

    /// 消費側が `Silence` を見た直後に呼ぶ `rearm_after` を、Fire 腕が飲み込まないこと。
    ///
    /// 消費側（`HeuristicRegistry`）はゲート規則 3 で沈黙を抑止したとき `rearm_after` を呼ぶ。
    /// その呼び出しが `watcher_alive.store(false)` より**前**に着弾すると `swap` が `true` を
    /// 返して空振りし、直後にウォッチャ自身も return する —— **ウォッチャ 0 本・再 arm 無し**で、
    /// その沈黙期間は二度と評価されない。単一スレッドの harness ではこの interleaving を
    /// 偶然には踏まないが、本番のマルチスレッドランタイムでは踏みうる。
    ///
    /// 手当ては順序である: `store(false)` を `tx.send` より前に置く。`store` は `send` に
    /// happens-before し、`send` は受信側の `recv` に happens-before するので、
    /// **消費側の `swap` は必ず `false` を見る**（アーキテクチャ依存の論法ではない）。
    /// この seam がその順序の唯一の観測点である —— 順序を戻すとこのテストが赤くなる。
    #[tokio::test(start_paused = true)]
    async fn a_rearm_from_the_consumer_is_not_swallowed_by_the_firing_watcher() {
        let (clock, act, mut rx) = setup(5_000);
        act.record_output(0);
        tokio::task::yield_now().await;

        let hook_ran = Arc::new(AtomicBool::new(false));
        let _seam = {
            let act = Arc::clone(&act);
            let hook_ran = Arc::clone(&hook_ran);
            install_fire_arm_seam(move || {
                // 消費側が「猶予の残り」を渡して再評価を予約する動きと同じ
                act.rearm_after(1_000);
                hook_ran.store(true, Ordering::SeqCst);
            })
        };

        advance(&clock, 5_500).await;
        assert!(
            hook_ran.load(Ordering::SeqCst),
            "seam が走っていない —— このテストは何も測っていない"
        );
        assert!(rx.try_recv().is_ok(), "1 本目の沈黙が出ていない");
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        advance(&clock, 1_100).await;
        assert_eq!(
            rx.try_recv().unwrap(),
            HeuristicEvent::Silence {
                session_id: "s1".into()
            },
            "消費側の rearm_after が空振りし、ウォッチャが消えている"
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
