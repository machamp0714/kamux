//! ヒューリスティック検知の統括。セッションの登録/解除、イベント消費ループ、診断を持つ。
//!
//! 消費ループは必ず `heuristic_transition` ゲートを通してから `RuntimeStateSink` へ
//! **入力**（`StateInput`）を渡す。次の状態は M2-1 の遷移表が決める（契約 §41.4）。
//! ゲートを迂回する経路をこのモジュールの外に作ってはならない。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;
use tokio::sync::mpsc;

use super::activity::SessionActivity;
use super::clock::Clock;
use super::gate::{heuristic_transition, HeuristicContext, HeuristicInput};
use super::hook_liveness::{HookLiveness, HookLivenessTracker};
use super::{HeuristicEvent, RuntimeStateSink, MAX_SILENCE_TIMEOUT_SECS, MIN_SILENCE_TIMEOUT_SECS};
use crate::model::CliKind;

/// 猶予切れの「直後」に沈黙を評価し直すための上乗せ（ms）。
///
/// 猶予の残りちょうどで起こすと、境界（`now - spawned_at == HOOK_GRACE_MS`）に
/// 着地できるかが時計の粒度に依存し、まだ `Pending` と判定される余地が残る。
/// **正の値でなければならない** —— 0 だと「即 Fire → ゲート規則 3 で抑止 →
/// 遅延 0 で再 arm」の busy loop になり、契約 §0「アイドル CPU ほぼ 0%」が壊れる。
const REARM_MARGIN_MS: u64 = 100;
const _: () = assert!(REARM_MARGIN_MS > 0);

/// `cli_kind` ごとのヒューリスティック既定値（設計 §4.6）。
///
/// shell だけ既定オフ。対話シェルは補完失敗のたびに BEL を鳴らし、
/// ユーザーが見ている前提の画面なので、常時 🟡 になると通知が無意味になる。
pub fn default_heuristics_enabled(cli_kind: CliKind) -> bool {
    !matches!(cli_kind, CliKind::Shell)
}

/// 設定値を許容範囲へ丸める。0 を通すとウォッチャが busy loop になるため必須。
pub fn clamp_timeout_secs(secs: u32) -> u32 {
    secs.clamp(MIN_SILENCE_TIMEOUT_SECS, MAX_SILENCE_TIMEOUT_SECS)
}

/// 抑止した沈黙をもう一度評価させるまでの遅延（ms）。**どんな入力でも正**である。
///
/// 正であることは飽和加算と正の上乗せで構造的に保証する ——
/// `remaining_grace_ms` が正の値しか返さないことにも依存させない。
/// **抑止された沈黙は `RuntimeStateSink` に 1 件も届かないので、
/// ここが 0 になったときの busy loop は消費履歴の件数では観測できない。**
fn rearm_delay_ms(remaining_grace_ms: i64) -> u64 {
    u64::try_from(remaining_grace_ms)
        .unwrap_or(0)
        .saturating_add(REARM_MARGIN_MS)
}

/// 設定画面向けのセッション単位ステータス。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionHookStatus {
    pub session_id: String,
    pub cli_kind: CliKind,
    pub liveness: HookLiveness,
    pub last_hook_at: Option<i64>,
    /// 現時点でヒューリスティックが実際に働いているか（ゲートを通る状態か）
    pub heuristics_active: bool,
}

struct RegistryEntry {
    enabled: bool,
    activity: Arc<SessionActivity>,
}

pub struct HeuristicRegistry {
    clock: Arc<dyn Clock>,
    sink: Arc<dyn RuntimeStateSink>,
    liveness: HookLivenessTracker,
    entries: Mutex<HashMap<String, RegistryEntry>>,
    tx: mpsc::UnboundedSender<HeuristicEvent>,
    rt: tokio::runtime::Handle,
}

impl HeuristicRegistry {
    /// `rt` は消費ループとウォッチャを走らせるランタイムハンドル。
    /// 本番は `tauri::async_runtime::handle().inner().clone()` を渡す。
    ///
    /// ここで `tokio::spawn` / `Handle::current()` を呼んではならない。
    /// この関数は `tauri::Builder::setup()` からも呼ばれうるが、
    /// そこはランタイム文脈の外なので両者は panic する。
    /// テストは常にランタイム内から呼ぶため、この panic は `cargo test` では絶対に再現しない。
    pub fn new(
        clock: Arc<dyn Clock>,
        sink: Arc<dyn RuntimeStateSink>,
        rt: tokio::runtime::Handle,
    ) -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let registry = Arc::new(Self {
            clock: Arc::clone(&clock),
            sink,
            liveness: HookLivenessTracker::new(clock),
            entries: Mutex::new(HashMap::new()),
            tx,
            rt: rt.clone(),
        });
        let consumer = Arc::clone(&registry);
        rt.spawn(async move {
            let mut rx = rx;
            while let Some(event) = rx.recv().await {
                consumer.handle(&event);
            }
        });
        registry
    }

    fn handle(&self, event: &HeuristicEvent) {
        let session_id = event.session_id();

        let (enabled, activity) = match self.lock().get(session_id) {
            Some(e) => (e.enabled, Arc::clone(&e.activity)),
            None => return, // 登録解除済み。取りこぼしたイベントは捨てる
        };

        let Some(current) = self.sink.current(session_id) else {
            return;
        };

        let ctx = HeuristicContext {
            current,
            heuristics_enabled: enabled,
            hook_liveness: self.liveness.liveness(session_id),
        };

        // 契約 §41.4: ゲートが返すのは状態機械への**入力**。次の状態は M2-1 が決める。
        match heuristic_transition(ctx, event.input()) {
            Some(input) => self.sink.send(session_id, input),
            None => self.rearm_if_the_grace_window_swallowed_it(&ctx, event, &activity),
        }
    }

    /// ゲート規則 3（`hook_liveness == Pending`）で**沈黙**を抑止したとき、
    /// 猶予切れの直後にもう一度評価させる。
    ///
    /// 呼ばないと、`silence_timeout_secs` が猶予（`HOOK_GRACE_MS`）より短い claude
    /// セッションで沈黙推定が二度と発火しない —— 沈黙イベントは猶予の中に着弾して
    /// ここで消えるが、`watch_silence` は Fire でウォッチャを終えるため、
    /// **出力が再開しない限り次のウォッチャが立たない。**
    ///
    /// BEL の抑止では呼ばない。**理由は「同じ `record_output` が既に次のウォッチャを
    /// 立てているので外しても等価」ではない —— それは誤りだった**（round m3-3-t9-r1 の記述を撤回）。
    /// `record_output` は `tx.send(Bel)` を `watcher_alive.swap(true)` より**先**に行うので、
    /// 消費側が `Bel` を見た瞬間にはウォッチャがまだ確立していない窓がある
    /// （Fire 腕で閉じた窓の鏡像である）。そこで再 arm すると消費側が先にフラグを取り、
    /// `record_output` 側の `spawn_watcher(0)` は `swap` が `true` を返して走らない ——
    /// **条件を外すと振る舞いは変わる。**
    ///
    /// 呼ばない理由は「BEL は出力があった証拠であって、沈黙の再評価を要求しない」である。
    /// 再評価が要るのは沈黙が猶予に飲まれたときだけで、BEL の抑止はその状況を作らない。
    ///
    /// **実測とコードからの導出の切り分け**:
    /// - 実測: `activity.rs` の送信が `swap` より前であること（現物のコード）。
    ///   条件を外す変異が `tests::a_suppressed_bel_does_not_re_arm_the_watcher` で
    ///   赤になること（round m3-3-t9-fix-r1）
    /// - 導出（未実測）: 本番のマルチスレッドランタイムでその窓を実際に踏む頻度と、
    ///   踏んだときの遅延が `remaining_grace + REARM_MARGIN_MS` になること
    ///
    /// 抑止の理由が規則 3 であることは `heuristics_enabled && hook_liveness == Pending`
    /// で判定できる —— `gate::heuristic_transition` は規則 1（セッション単位のオフ）→
    /// 規則 2/3（`Healthy` / `Pending`）→ 規則 4 以降の順に見るので、規則 1 を通過して
    /// `Pending` なら止めたのは規則 3 である。**この推論は gate.rs の規則順に依存している。**
    fn rearm_if_the_grace_window_swallowed_it(
        &self,
        ctx: &HeuristicContext,
        event: &HeuristicEvent,
        activity: &Arc<SessionActivity>,
    ) {
        if event.input() != HeuristicInput::Silence
            || !ctx.heuristics_enabled
            || ctx.hook_liveness != HookLiveness::Pending
        {
            return;
        }
        // 猶予切れの判定は `HookLivenessTracker` が正典。ここに写さない。
        //
        // 🔴 `None` でも再 arm する。`ctx.hook_liveness` と残り猶予は**別々に時計を読む**
        // （`liveness()` と `remaining_grace_ms()` がそれぞれ lock と `now_ms()` を取る）ので、
        // 2 つの読みの間に猶予境界をまたぐと「`Pending` だったのに残りは `None`」が起こる。
        // そこで return すると、A-1 が防ごうとしている「沈黙推定が二度と発火しない」に落ちる。
        // 猶予は既に切れているので最小の上乗せで起こし直せばよい。
        //
        // **ループしない根拠**（コードからの導出。実測は下記テストの後半 2 assert）:
        // `None` になる原因は「猶予切れ（`Unreachable`）」「hook 到着（`Healthy`）」
        // 「resume で `cli_kind` が変わった（`NotApplicable`）」「エントリ消滅」など
        // だが、いずれも次の評価では規則 3 を通らない（規則 4 以降が `Some` を返して
        // `send` するか、エントリが無くて early-return する）ためここへ二度は来ない。
        // **原因を数え上げて網羅を主張しているのではない** —— 効いているのは
        // 「次の評価は規則 3 を通らない」の一点である。resume で猶予をやり直した場合は
        // `register` が古い `SessionActivity` を止めているので、この再 arm は空振りする。
        let delay = match self.liveness.remaining_grace_ms(event.session_id()) {
            Some(remaining) => rearm_delay_ms(remaining),
            // 直書きの `REARM_MARGIN_MS` にしない —— 正であることの保証（飽和加算と
            // const assert）を `rearm_delay_ms` の 1 箇所に保つ
            None => rearm_delay_ms(0),
        };
        activity.rearm_after(delay);
    }

    /// PTY spawn 時に呼ぶ。返り値の `SessionActivity` を `AgentOutputObserver` に渡す。
    pub fn register(
        &self,
        session_id: &str,
        cli_kind: CliKind,
        enabled: bool,
        timeout_secs: u32,
    ) -> Arc<SessionActivity> {
        let timeout_ms = (clamp_timeout_secs(timeout_secs) as u64) * 1_000;
        let activity = SessionActivity::new(
            session_id.to_string(),
            Arc::clone(&self.clock),
            self.tx.clone(),
            self.rt.clone(),
            enabled,
            timeout_ms,
        );
        self.liveness.on_spawn(session_id, cli_kind);
        // 一時 guard は文の終わりで落ちる。`if let` の被検査式に置くと
        // 下の `reconfigure` までロックを持ち越すので、束縛を分ける
        let previous = self.lock().insert(
            session_id.to_string(),
            RegistryEntry {
                enabled,
                activity: Arc::clone(&activity),
            },
        );
        // resume（`unregister` を挟まない再 `register`）。押し出された古い
        // `SessionActivity` を必ず止める —— `HookLivenessTracker::on_spawn` は
        // 再 spawn で猶予をリセットする（resume 対応）ので、この呼ばれ方は起こりうる。
        // 止めないと古いウォッチャが**古い活動時刻**を基準に `Silence` を送り続け、
        // 消費ループは新しいエントリを引くため resume 直後のセッションが即
        // `SilenceTimeout` を受ける。停止の手順は `unregister` と同じ。
        //
        // `insert` の前に止めても保証は変わらない —— `SessionActivity::reconfigure` は
        // フラグの store と `notify_waiters` だけで、**停止は非同期**である
        // （旧ウォッチャはどちらの順序でも次の起床まで生きる）。差は `insert` と停止の
        // 間の数命令ぶんに縮むだけで、既にチャンネルへ載ったイベントを回収できない点も同じ
        if let Some(old) = previous {
            old.activity
                .reconfigure(false, (MIN_SILENCE_TIMEOUT_SECS as u64) * 1_000);
        }
        activity
    }

    /// PTY exit / stop_session で呼ぶ。
    pub fn unregister(&self, session_id: &str) {
        if let Some(entry) = self.lock().remove(session_id) {
            // 走っているウォッチャを次の起床で終了させる
            entry
                .activity
                .reconfigure(false, (MIN_SILENCE_TIMEOUT_SECS as u64) * 1_000);
        }
        self.liveness.on_exit(session_id);
    }

    /// `update_session` から呼ぶ。実行中のセッションへ即座に反映する。
    pub fn reconfigure(&self, session_id: &str, enabled: bool, timeout_secs: u32) {
        let timeout_ms = (clamp_timeout_secs(timeout_secs) as u64) * 1_000;
        let mut guard = self.lock();
        let Some(entry) = guard.get_mut(session_id) else {
            return;
        };
        let was_enabled = entry.enabled;
        entry.enabled = enabled;
        // 🔴 先に有効化を反映する。逆順（下の `rearm_after` を先に呼ぶ）にすると、
        // 立てた新しいウォッチャが先頭の `enabled` 判定で `false` を見て降りうる ——
        // disabled 腕の再チェックが救うのは `enabled.store` がその再チェックより前に
        // 着弾した場合だけで、後になればウォッチャは消え、誰も立て直さない。
        //
        // **観測が無い。実測と導出を分けて書く**:
        // - 実測: 逆順にする変異は round m3-3-t9-r1 と m3-3-t9-rev-r1 の 2 度とも全緑
        //   だった（= 現行のテスト集合はこの順序を区別しない）
        // - コードからの導出（未実測）: spawn したタスクが最初に polled されるのは
        //   `reconfigure` が返った後なので、逆順の帰結（ウォッチャが消える）が
        //   単一スレッド・仮想時間の harness には現れない
        //
        // ⚠️ 以前ここに書いていた「観測できない」および報告側の
        // 「seam を置くと `HeuristicRegistry` の外形を変えることになる」は**撤回する** ——
        // `activity.rs` は同型の窓を `#[cfg(test)]` の thread_local seam で
        // 外形を変えずに 3 本閉じており、後者は事実に反する。観測を置いていないのは
        // lane-controller の裁定（M-1 は round m3-3-t9-fix-r1 の scope 外）による。
        // 現状この順序を守らせているのはこのコメントだけである。入れ替えないこと。
        entry.activity.reconfigure(enabled, timeout_ms);
        let re_enabled = (!was_enabled && enabled).then(|| Arc::clone(&entry.activity));
        // ロックを持ったまま spawn 経路へ入らない
        drop(guard);

        // 無効だった間はウォッチャが 1 本も居ない。ここで立て直さないと、
        // 再有効化しても**次の出力チャンクが来るまで**沈黙が一切評価されない。
        // 活動時刻は据え置きなので、長く無音だったセッションはすぐに沈黙が成立する
        // —— 無効化していた間の無音は「出力が無かった」事実そのものである。
        if let Some(activity) = re_enabled {
            activity.rearm_after(0);
        }
    }

    /// hooks_srv が hook を受信したときに呼ぶ。`Healthy` へ昇格させる。
    pub fn note_hook(&self, session_id: &str) {
        self.liveness.on_hook(session_id);
    }

    pub fn diagnostics(&self) -> Vec<SessionHookStatus> {
        let guard = self.lock();
        self.liveness
            .snapshot()
            .into_iter()
            .map(|(session_id, cli_kind, liveness, last_hook_at)| {
                let enabled = guard.get(&session_id).map(|e| e.enabled).unwrap_or(false);
                let heuristics_active =
                    enabled && !matches!(liveness, HookLiveness::Healthy | HookLiveness::Pending);
                SessionHookStatus {
                    session_id,
                    cli_kind,
                    liveness,
                    last_hook_at,
                    heuristics_active,
                }
            })
            .collect()
    }

    /// 毒された Mutex でも panic しない（契約 §0）
    fn lock(&self) -> MutexGuard<'_, HashMap<String, RegistryEntry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RuntimeState;
    use crate::session::heuristics::clock::TestClock;
    use crate::session::heuristics::{FakeSink, HOOK_GRACE_MS};
    use crate::session::runtime_state::StateInput;
    use std::time::Duration;

    /// spawn したタスクが起きて次の sleep を登録し終えるまで回す。
    /// 回数に意味は無い（1 回では足りない経路があるので余裕を取ってある）。
    async fn settle() {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }

    async fn advance(clock: &TestClock, ms: u64) {
        clock.advance_ms(ms as i64);
        tokio::time::advance(Duration::from_millis(ms)).await;
        settle().await;
    }

    fn setup(
        initial: &[(&str, RuntimeState)],
    ) -> (TestClock, Arc<FakeSink>, Arc<HeuristicRegistry>) {
        let clock = TestClock::new(0);
        let sink = Arc::new(FakeSink::new(initial));
        let reg = HeuristicRegistry::new(
            Arc::new(clock.clone()),
            sink.clone(),
            tokio::runtime::Handle::current(),
        );
        (clock, sink, reg)
    }

    /// 消費ループが**何回走ったか**を数える `FakeSink` のラッパ。
    ///
    /// **抑止されたイベントは `sent()` に 1 件も現れない。** そのため
    /// 「再 arm の遅延が短すぎて評価を繰り返している（= 契約 §0 のアイドル CPU を
    /// 食い潰している）」ことは消費履歴では観測できない。`current` は `handle` 1 回に
    /// つきちょうど 1 回呼ばれるので、ここが評価回数の観測点になる。
    struct CountingSink {
        inner: FakeSink,
        evaluations: std::sync::atomic::AtomicUsize,
    }

    impl CountingSink {
        fn new(initial: &[(&str, RuntimeState)]) -> Self {
            Self {
                inner: FakeSink::new(initial),
                evaluations: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        /// 消費ループがゲートを評価した回数（抑止された分も含む）
        fn evaluations(&self) -> usize {
            self.evaluations.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn sent(&self) -> Vec<(String, StateInput)> {
            self.inner.sent()
        }
    }

    impl RuntimeStateSink for CountingSink {
        fn current(&self, session_id: &str) -> Option<RuntimeState> {
            self.evaluations
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.current(session_id)
        }

        fn send(&self, session_id: &str, input: StateInput) {
            self.inner.send(session_id, input);
        }
    }

    /// `SessionActivity` の内部フラグを production の `Debug` 越しに読む。
    ///
    /// **なぜイベントの不在では測れないか**: registry が作った `SessionActivity` の
    /// `tx` は registry が握っており、届いたイベントは `handle()` が処理する。
    /// `handle()` はエントリが消えていると early-return するので、
    /// `sink.sent()` は**ウォッチャが生きていようが死んでいようが空になる** ——
    /// `unregister` の停止機構をイベントの不在で測ることは原理的にできない
    /// （実測: 停止機構を丸ごと削る変異は round m3-3-t9-rev-r1 で全緑だった）。
    fn assert_activity(act: &Arc<SessionActivity>, enabled: bool, watcher_alive: bool, msg: &str) {
        let snap = format!("{act:?}");
        assert!(
            snap.contains(&format!("enabled: {enabled}")),
            "{msg} —— enabled が {enabled} ではない: {snap}"
        );
        assert!(
            snap.contains(&format!("watcher_alive: {watcher_alive}")),
            "{msg} —— watcher_alive が {watcher_alive} ではない: {snap}"
        );
    }

    fn setup_counting(
        initial: &[(&str, RuntimeState)],
    ) -> (TestClock, Arc<CountingSink>, Arc<HeuristicRegistry>) {
        let clock = TestClock::new(0);
        let sink = Arc::new(CountingSink::new(initial));
        let reg = HeuristicRegistry::new(
            Arc::new(clock.clone()),
            sink.clone(),
            tokio::runtime::Handle::current(),
        );
        (clock, sink, reg)
    }

    #[test]
    fn defaults_are_on_except_for_shell() {
        assert!(default_heuristics_enabled(CliKind::Claude));
        assert!(default_heuristics_enabled(CliKind::Codex));
        assert!(default_heuristics_enabled(CliKind::Custom));
        assert!(!default_heuristics_enabled(CliKind::Shell));
    }

    /// 範囲外の設定値は許容範囲へ丸める。0 を通すとウォッチャが busy loop になる。
    #[test]
    fn clamp_maps_out_of_range_values_onto_the_boundaries() {
        assert_eq!(clamp_timeout_secs(0), MIN_SILENCE_TIMEOUT_SECS);
        assert_eq!(
            clamp_timeout_secs(MIN_SILENCE_TIMEOUT_SECS - 1),
            MIN_SILENCE_TIMEOUT_SECS
        );
        assert_eq!(
            clamp_timeout_secs(MAX_SILENCE_TIMEOUT_SECS + 1),
            MAX_SILENCE_TIMEOUT_SECS
        );
        assert_eq!(clamp_timeout_secs(u32::MAX), MAX_SILENCE_TIMEOUT_SECS);
        // 範囲内の値は 1 秒も動かさない
        assert_eq!(
            clamp_timeout_secs(MIN_SILENCE_TIMEOUT_SECS),
            MIN_SILENCE_TIMEOUT_SECS
        );
        assert_eq!(clamp_timeout_secs(30), 30);
        assert_eq!(
            clamp_timeout_secs(MAX_SILENCE_TIMEOUT_SECS),
            MAX_SILENCE_TIMEOUT_SECS
        );
    }

    /// 再 arm の遅延は**どんな入力でも正**でなければならない。
    /// 0 を返すと「即 Fire → ゲート規則 3 で抑止 → 遅延 0 で再 arm」の busy loop になり、
    /// 契約 §0「アイドル CPU ほぼ 0%」が壊れる。
    /// **沈黙イベントは抑止されると `sink` に 1 件も届かないので、
    /// この spin は消費履歴の件数では観測できない。ここが唯一の観測点である。**
    #[test]
    fn the_rearm_delay_is_always_positive() {
        for remaining in [i64::MIN, -1, 0, 1, 14_500, i64::MAX] {
            assert!(
                rearm_delay_ms(remaining) > 0,
                "remaining={remaining} で遅延 0 が返った（busy loop になる）"
            );
        }
        // 猶予の残りは待ち時間として素直に反映される
        assert!(rearm_delay_ms(14_500) > 14_500);
    }

    #[tokio::test(start_paused = true)]
    async fn a_custom_cli_going_silent_becomes_idle() {
        let (clock, sink, reg) = setup(&[("s1", RuntimeState::Running)]);
        let act = reg.register("s1", CliKind::Custom, true, 30);
        act.record_output(0);
        tokio::task::yield_now().await;

        advance(&clock, 31_000).await;
        assert_eq!(
            sink.sent(),
            vec![("s1".to_string(), StateInput::SilenceTimeout)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_bel_makes_a_custom_cli_wait_for_input() {
        let (_c, sink, reg) = setup(&[("s1", RuntimeState::Running)]);
        let act = reg.register("s1", CliKind::Custom, true, 30);
        act.record_output(1);
        settle().await;

        assert_eq!(
            sink.sent(),
            vec![("s1".to_string(), StateInput::BelDetected)]
        );
    }

    /// hooks が健全なら推定は 1 件も適用されない。
    ///
    /// **`sent()` の空だけでは足りない。** ゲート規則 2 で抑止された評価は
    /// `RuntimeStateSink` に 1 件も届かないので、抑止 → 再 arm → 抑止…… の
    /// busy loop（契約 §0「アイドル CPU ほぼ 0%」）が `sent()` では原理的に見えない。
    /// 再 arm ガードの `hook_liveness == Pending` 条件を落とすと `Healthy` の沈黙が
    /// `rearm_delay_ms(0)` の腕へ落ちてこのループに入る（実測: 落とす変異は
    /// round m3-3-t9-rerev-r1 の D1 = 119/119 全緑だった）。そこで `CountingSink` で
    /// 評価回数を数える。
    #[tokio::test(start_paused = true)]
    async fn a_claude_session_with_healthy_hooks_is_never_touched() {
        let (clock, sink, reg) = setup_counting(&[("s1", RuntimeState::Running)]);
        let act = reg.register("s1", CliKind::Claude, true, 30);
        reg.note_hook("s1"); // SessionStart 相当
        act.record_output(1);
        tokio::task::yield_now().await;

        advance(&clock, 120_000).await;
        assert!(
            sink.sent().is_empty(),
            "hooks が生きている間は推定を一切適用しない"
        );

        // 抑止した評価を再 arm し続けていないこと（契約 §0）。
        // 🔴 小刻みに進めないとこの観測は空振りする —— 大きな advance を 1 回だけ打つと
        // 再 arm の sleep が満了せず、ループしていても evaluations が増えない。
        //
        // 測っているのは「繰り返しに**上限が無い**」ことである（実測: 条件を落とすと
        // 進めるたびに +1 で増え続け、5 歩で 2 → 7 になる）。刻み幅は harness の
        // `advance` 粒度が決めるので、**この数値からループの周期は読めない** ——
        // 100 ms 周期はコードから導いた未実測の帰結である。
        for _ in 0..5 {
            advance(&clock, 1_000).await;
        }
        assert_eq!(sink.evaluations(), 2, "抑止された評価を繰り返している");
    }

    #[tokio::test(start_paused = true)]
    async fn a_claude_session_inside_the_grace_window_is_not_touched() {
        let (clock, sink, reg) = setup(&[("s1", RuntimeState::Running)]);
        let act = reg.register("s1", CliKind::Claude, true, 30);
        act.record_output(1); // 猶予中の BEL
        advance(&clock, 100).await;
        assert!(sink.sent().is_empty(), "猶予中は hook を待つ");
    }

    #[tokio::test(start_paused = true)]
    async fn a_claude_session_without_hooks_falls_back_after_the_grace_window() {
        // 設計書 §12「hooks 不達 → 汎用ヒューリスティックへ自動フォールバック」
        let (clock, sink, reg) = setup(&[("s1", RuntimeState::Running)]);
        let act = reg.register("s1", CliKind::Claude, true, 30);
        act.record_output(0);
        tokio::task::yield_now().await;

        advance(&clock, 31_000).await;
        assert_eq!(
            sink.sent(),
            vec![("s1".to_string(), StateInput::SilenceTimeout)]
        );
    }

    /// **A-1 の観測点**: 沈黙タイムアウトが猶予（`HOOK_GRACE_MS`）より短い claude セッション。
    ///
    /// 沈黙イベントが猶予の中に着弾してゲート規則 3 で消えるが、`watch_silence` は
    /// Fire でウォッチャを終える。出力が再開しない限り次のウォッチャは立たないので、
    /// 消費ループが猶予切れ後の再評価を予約しなければ**沈黙推定は二度と発火しない**。
    #[tokio::test(start_paused = true)]
    async fn a_silence_swallowed_by_the_grace_window_is_re_evaluated_once_the_grace_expires() {
        let (clock, sink, reg) = setup_counting(&[("s1", RuntimeState::Running)]);
        // 5 秒 < 猶予 20 秒。沈黙が先に成立する
        let act = reg.register("s1", CliKind::Claude, true, 5);
        act.record_output(0); // 出力はここ 1 回きり
        settle().await;

        advance(&clock, 5_500).await;
        assert!(
            sink.sent().is_empty(),
            "猶予の中の沈黙はゲート規則 3 で抑止される"
        );

        advance(&clock, 10_000).await; // t=15.5s —— まだ猶予の中
        assert!(sink.sent().is_empty(), "猶予はまだ切れていない");

        advance(&clock, 5_000).await; // t=20.5s —— 猶予切れ
        assert_eq!(
            sink.sent(),
            vec![("s1".to_string(), StateInput::SilenceTimeout)],
            "猶予切れの後に再評価されていない（消費ループが再 arm していない）"
        );

        // 猶予をまたいで報告は 1 件きり
        advance(&clock, 60_000).await;
        assert_eq!(sink.sent().len(), 1, "沈黙 1 回につき報告は 1 件のはず");

        // **再 arm の遅延が効いていることの観測点。** 猶予 20 秒の間に評価するのは
        // 「抑止された 1 回」と「猶予切れ後の 1 回」だけである。遅延を 0 や
        // 猶予より短い値にすると、抑止 → 即再評価 → 抑止…… を繰り返して
        // ここが跳ね上がる（`sent()` は 1 件のままなので件数では見えない）。
        assert_eq!(
            sink.evaluations(),
            2,
            "消費ループが余分に走っている（再 arm の遅延が短すぎる）"
        );
    }

    /// 抑止された **BEL** では再 arm しない（再 arm は沈黙の抑止に限る）。
    ///
    /// `record_output` は `tx.send(Bel)` を `watcher_alive.swap(true)` より先に行うので、
    /// **消費側が `Bel` を見た瞬間にはウォッチャがまだ立っていない窓がある。**
    /// その窓で再 arm すると消費側が先にフラグを取り、`record_output` 側の
    /// `spawn_watcher(0)` は走らない —— ウォッチャが遅延つきで立ち、
    /// `timeout < grace` のセッションでは最初の沈黙評価が遅れる。
    ///
    /// その interleaving を、消費ループの本体（`handle`）を直接呼んで決定的に作る。
    /// **`record_output` 経由では作れない** —— 単一スレッドの harness では
    /// `record_output` が `swap` まで走り切ってから消費ループが動くので、
    /// 変異版でも `rearm_after` が no-op になり判別できない
    /// （実測: round m3-3-t9-rev-r1 の変異 S3 = 115/115 全緑）。
    #[tokio::test(start_paused = true)]
    async fn a_suppressed_bel_does_not_re_arm_the_watcher() {
        let (_c, sink, reg) = setup(&[("s1", RuntimeState::Running)]);
        // 猶予（20 秒）より短いタイムアウト = A-1 が効く条件。出力は与えないので
        // ウォッチャは 1 本も居ない（= `record_output` が swap する前と同じ状態）
        let act = reg.register("s1", CliKind::Claude, true, 5);
        assert_activity(
            &act,
            true,
            false,
            "前提が崩れている: 出力を与えていないのにウォッチャが立っている",
        );

        reg.handle(&HeuristicEvent::Bel {
            session_id: "s1".into(),
        });

        assert!(
            sink.sent().is_empty(),
            "前提: 猶予中の BEL はゲート規則 3 で抑止されるはず"
        );
        assert_activity(
            &act,
            true,
            false,
            "抑止された BEL で再 arm している（沈黙の初回評価が猶予切れ後まで遅れる）",
        );
    }

    /// `handle()` が猶予を **2 回**読む（`liveness()` と `remaining_grace_ms()` が
    /// それぞれ別に lock と `now_ms()` を取る）ことを利用して、2 つの読みの間で
    /// 猶予境界をまたぐ interleaving を決定的に作る時計。
    ///
    /// `arm()` してから最初の 1 回だけ現在時刻をそのまま返し、返した**直後**に
    /// 内側の時計を 1 ミリ秒進める。以後は素通しになる。
    struct BoundaryCrossingClock {
        inner: TestClock,
        armed: std::sync::atomic::AtomicBool,
        crossed: std::sync::atomic::AtomicBool,
    }

    impl BoundaryCrossingClock {
        fn new(inner: TestClock) -> Self {
            Self {
                inner,
                armed: std::sync::atomic::AtomicBool::new(false),
                crossed: std::sync::atomic::AtomicBool::new(false),
            }
        }

        fn arm(&self) {
            self.armed.store(true, std::sync::atomic::Ordering::SeqCst);
        }

        /// 差し込みが実際に起きたか。起きていなければテストは何も測っていない（群 P）。
        fn crossed(&self) -> bool {
            self.crossed.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl Clock for BoundaryCrossingClock {
        fn now_ms(&self) -> i64 {
            use std::sync::atomic::Ordering::SeqCst;
            let now = self.inner.now_ms();
            if self.armed.swap(false, SeqCst) {
                self.inner.advance_ms(1);
                self.crossed.store(true, SeqCst);
            }
            now
        }
    }

    /// **M-3 の観測点**: `ctx.hook_liveness` と `remaining_grace_ms` は別々に時計を読む。
    /// 2 つの読みの間に猶予境界をまたぐと、`ctx` は `Pending`・残り猶予は `None` になる。
    /// そこで再 arm せずに return すると、**A-1 が防ごうとしている「沈黙推定が二度と
    /// 発火しない」そのものに落ちる** —— 抑止された沈黙は消え、`watch_silence` は
    /// Fire で降りているので、出力が再開しない限り次のウォッチャが立たない。
    ///
    /// 後半 2 つの assert は「この経路がループしない」ことの観測点である
    /// （再 arm 後の評価は `Unreachable` になるので規則 3 を通らず、二度目は来ない）。
    #[tokio::test(start_paused = true)]
    async fn a_grace_boundary_crossed_between_the_two_clock_reads_still_re_arms() {
        let inner = TestClock::new(0);
        let clock = Arc::new(BoundaryCrossingClock::new(inner.clone()));
        let sink = Arc::new(CountingSink::new(&[("s1", RuntimeState::Running)]));
        let reg = HeuristicRegistry::new(
            clock.clone(),
            sink.clone(),
            tokio::runtime::Handle::current(),
        );
        // 猶予（20 秒）より短いタイムアウト = A-1 が効く条件。
        // 出力を与えないのでウォッチャは 1 本も居ない
        let act = reg.register("s1", CliKind::Claude, true, 5);
        assert_activity(
            &act,
            true,
            false,
            "前提が崩れている: 出力を与えていないのにウォッチャが立っている",
        );

        inner.advance_ms(HOOK_GRACE_MS - 1); // 猶予切れの 1 ミリ秒前
        clock.arm();
        reg.handle(&HeuristicEvent::Silence {
            session_id: "s1".into(),
        });

        assert!(
            clock.crossed(),
            "境界をまたぐ差し込みが起きていない —— このテストは何も測っていない"
        );
        // これが空であることが「1 度目の読みは Pending だった」ことの証拠である
        //（`Unreachable` を読んでいれば規則 3 を通過して SilenceTimeout が飛ぶ）
        assert!(sink.sent().is_empty(), "前提: 1 度目の読みが猶予の中に無い");
        assert_activity(
            &act,
            true,
            true,
            "2 つの時計読みの間に猶予が切れると再 arm を取りこぼす",
        );
        assert_eq!(sink.evaluations(), 1);

        // 再 arm したウォッチャが猶予切れ後の沈黙を報告する
        settle().await; // 立てたウォッチャに遅延を登録させる
        inner.advance_ms(REARM_MARGIN_MS as i64 + 100);
        tokio::time::advance(Duration::from_millis(REARM_MARGIN_MS + 100)).await;
        settle().await;
        assert_eq!(
            sink.sent(),
            vec![("s1".to_string(), StateInput::SilenceTimeout)],
            "再 arm したウォッチャが沈黙を報告していない"
        );

        // 二度と評価し直さない（この経路が busy loop にならないことの観測点）
        inner.advance_ms(120_000);
        tokio::time::advance(Duration::from_millis(120_000)).await;
        settle().await;
        assert_eq!(sink.evaluations(), 2, "再 arm の後もループしている");
        assert_eq!(sink.sent().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_late_hook_stops_further_heuristics() {
        let (clock, sink, reg) = setup(&[("s1", RuntimeState::Running)]);
        let act = reg.register("s1", CliKind::Claude, true, 30);
        act.record_output(0);
        tokio::task::yield_now().await;
        advance(&clock, 31_000).await;
        assert_eq!(sink.sent().len(), 1);

        reg.note_hook("s1"); // 遅れて hook が届いた
        act.record_output(1);
        advance(&clock, 120_000).await;
        assert_eq!(sink.sent().len(), 1, "昇格後は推定を止める");
    }

    #[tokio::test(start_paused = true)]
    async fn silence_does_not_overwrite_a_waiting_input_session() {
        let (clock, sink, reg) = setup(&[("s1", RuntimeState::WaitingInput)]);
        let act = reg.register("s1", CliKind::Custom, true, 30);
        act.record_output(0);
        tokio::task::yield_now().await;

        advance(&clock, 31_000).await;
        assert!(sink.sent().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn an_exited_session_is_never_marked_idle() {
        let (clock, sink, reg) = setup(&[("s1", RuntimeState::Exited)]);
        let act = reg.register("s1", CliKind::Custom, true, 30);
        act.record_output(0);
        tokio::task::yield_now().await;

        advance(&clock, 31_000).await;
        assert!(sink.sent().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn reconfigure_reaches_the_running_activity() {
        let (clock, sink, reg) = setup(&[("s1", RuntimeState::Running)]);
        let act = reg.register("s1", CliKind::Custom, true, 300);
        act.record_output(0);
        tokio::task::yield_now().await;

        advance(&clock, 10_000).await;
        assert!(sink.sent().is_empty());

        reg.reconfigure("s1", true, 5);
        settle().await;
        assert_eq!(sink.sent().len(), 1);
    }

    /// **A-2 の観測点**: 無効だった間はウォッチャが 1 本も居ない。再有効化しても
    /// 消費ループが立て直さなければ、**次の出力チャンクが来るまで沈黙が一切評価されない。**
    ///
    /// `SessionActivity` の活動時刻は登録時刻のままなので、長く無効だったセッションは
    /// 再有効化した瞬間に沈黙が成立する。**それが意図した挙動である** ——
    /// 無効化していた間の無音は「出力が無かった」事実そのものだからである。
    /// だからこのテストは時間を 1 ミリ秒も進めずに `SilenceTimeout` を要求する。
    #[tokio::test(start_paused = true)]
    async fn re_enabling_a_session_evaluates_silence_without_waiting_for_new_output() {
        let (clock, sink, reg) = setup(&[("s1", RuntimeState::Running)]);
        let act = reg.register("s1", CliKind::Custom, false, 30); // 無効で登録
        act.record_output(0);
        advance(&clock, 60_000).await;
        assert!(sink.sent().is_empty(), "無効な間は何も推定しない");

        // 出力を 1 度も与えずに再有効化する
        reg.reconfigure("s1", true, 30);
        settle().await; // 時間は進めない

        assert_eq!(
            sink.sent(),
            vec![("s1".to_string(), StateInput::SilenceTimeout)],
            "再有効化しても次の出力まで沈黙が評価されていない"
        );
    }

    /// **A-2 の裏返し**: 既に有効なセッションの `reconfigure` は再 arm しない。
    ///
    /// 立て直しが要るのは「無効な間ウォッチャが 1 本も居なかった」場合だけである。
    /// 有効なままの設定変更は `SessionActivity::reconfigure` の起床通知で足りる ——
    /// 発火し終えて降りたウォッチャまで無条件に立て直すと、同じ抑止を何度も評価し直す。
    #[tokio::test(start_paused = true)]
    async fn reconfiguring_an_already_enabled_session_does_not_re_arm_a_finished_watcher() {
        let (clock, sink, reg) = setup_counting(&[("s1", RuntimeState::WaitingInput)]);
        let act = reg.register("s1", CliKind::Custom, true, 30);
        act.record_output(0);
        settle().await;

        // 沈黙は成立するが WaitingInput の方が情報量が多いので抑止される
        advance(&clock, 31_000).await;
        assert!(sink.sent().is_empty());
        assert_eq!(sink.evaluations(), 1, "抑止された評価が 1 回だけ起きる");

        reg.reconfigure("s1", true, 30); // true → true
        settle().await;
        assert_eq!(
            sink.evaluations(),
            1,
            "有効なままの設定変更で評価が増えている（再 arm の条件が広すぎる）"
        );
    }

    /// `unregister` は map から消すだけでなく、**走っているウォッチャを止める**。
    ///
    /// 🔴 `sink.sent()` の空はこの停止機構を 1 ミリも測っていない ——
    /// `handle()` はエントリが消えていると early-return するので、ウォッチャが
    /// 生きたまま `Silence` を送り続けても `sent()` は空のままである
    /// （実測: `entry.activity.reconfigure(false, ..)` を丸ごと削る変異は
    /// round m3-3-t9-rev-r1 で 115/115 全緑だった）。止まったことは
    /// `SessionActivity` 側を直接見るしかない。
    #[tokio::test(start_paused = true)]
    async fn unregister_stops_everything() {
        let (clock, sink, reg) = setup(&[("s1", RuntimeState::Running)]);
        let act = reg.register("s1", CliKind::Custom, true, 30);
        act.record_output(0);
        // `settle()` で `select!` まで到達させる。`notify_waiters` は**待機登録済みの
        // waiter だけ**を起こすので、ここが足りないと下の assert が変異と無関係に赤くなる
        settle().await;
        assert_activity(
            &act,
            true,
            true,
            "前提が崩れている: ウォッチャが 1 本も走っていない",
        );

        reg.unregister("s1");
        settle().await;
        // ここが `unregister` の停止機構の唯一の観測点である
        assert_activity(
            &act,
            false,
            false,
            "unregister が走っているウォッチャを止めていない",
        );

        advance(&clock, 120_000).await;
        assert!(sink.sent().is_empty());
        assert!(reg.diagnostics().is_empty());
    }

    /// `unregister` を挟まない再 `register`（resume）で、押し出された古い
    /// `SessionActivity` を止める。
    ///
    /// `HookLivenessTracker::on_spawn` は「既存エントリがあれば猶予をリセットする
    /// （resume 対応）」と明言し `on_spawn_resets_the_grace_window` で固定されている ——
    /// つまり同じ `session_id` の再 `register` は起こりうる呼ばれ方である。
    /// 止めないと、押し出された古いウォッチャが**古い活動時刻**を基準に `Silence` を送り、
    /// 消費ループは**新しい**エントリを引くので resume 直後のセッションが即
    /// `SilenceTimeout` を受ける。
    ///
    /// ⚠️ `CliKind::Claude` では判別しない。resume の `on_spawn` が猶予をやり直すため、
    /// 古いウォッチャの `Silence` が新しい猶予窓の中に着弾してゲート規則 3 で消え、
    /// **停止機構が無くても `sent()` が空になる。** `NotApplicable` になる `Custom` で測ること。
    #[tokio::test(start_paused = true)]
    async fn re_registering_the_same_session_stops_the_previous_activity() {
        let (clock, sink, reg) = setup(&[("s1", RuntimeState::Running)]);
        let old = reg.register("s1", CliKind::Custom, true, 30);
        old.record_output(0);
        settle().await;
        assert_activity(
            &old,
            true,
            true,
            "前提が崩れている: 古いウォッチャが走っていない",
        );

        advance(&clock, 25_000).await; // 沈黙成立まで残り 5 秒

        let new = reg.register("s1", CliKind::Custom, true, 30); // unregister を挟まない resume
        settle().await;
        assert_activity(
            &old,
            false,
            false,
            "押し出された古い activity が止まっていない",
        );

        // 新しい activity の基準は register 時刻（t=25s）。古い基準（t=0）のウォッチャが
        // 生き残っていると、ここで沈黙が成立して報告が飛ぶ
        advance(&clock, 10_000).await; // t=35s
        assert!(
            sink.sent().is_empty(),
            "古いウォッチャが古い活動時刻を基準に沈黙を報告している（resume 直後の誤検知）"
        );

        // 新しい activity は正しく働く
        new.record_output(0);
        settle().await;
        advance(&clock, 31_000).await;
        assert_eq!(
            sink.sent(),
            vec![("s1".to_string(), StateInput::SilenceTimeout)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn diagnostics_reports_per_session_status() {
        let (clock, _sink, reg) = setup(&[]);
        reg.register("s1", CliKind::Claude, true, 30);
        reg.register("s2", CliKind::Shell, false, 30);
        reg.note_hook("s1");
        advance(&clock, 25_000).await;

        let mut diag = reg.diagnostics();
        diag.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        assert_eq!(diag.len(), 2);

        assert_eq!(diag[0].session_id, "s1");
        assert_eq!(diag[0].cli_kind, CliKind::Claude);
        assert_eq!(diag[0].liveness, HookLiveness::Healthy);
        assert_eq!(diag[0].last_hook_at, Some(0));
        assert!(
            !diag[0].heuristics_active,
            "hooks 健全なら推定は動いていない"
        );

        assert_eq!(diag[1].session_id, "s2");
        assert_eq!(diag[1].cli_kind, CliKind::Shell);
        assert_eq!(diag[1].liveness, HookLiveness::NotApplicable);
        assert!(!diag[1].heuristics_active, "オフなら動いていない");
    }

    /// 猶予中（`Pending`）はまだ hook を待っている段階なので、推定は働いていない。
    ///
    /// **他の diagnostics のテストは `Healthy` / `NotApplicable` / `Unreachable` しか
    /// 通らないため、`heuristics_active` の判定から `Pending` を落とす変異を
    /// 1 本も捕まえられない**（実測: 落とす変異は 630 passed / 0 failed で全緑だった）。
    #[tokio::test(start_paused = true)]
    async fn diagnostics_does_not_mark_a_claude_session_inside_the_grace_window_as_active() {
        let (_c, _sink, reg) = setup(&[]);
        reg.register("s1", CliKind::Claude, true, 30);

        let diag = reg.diagnostics();
        assert_eq!(diag[0].liveness, HookLiveness::Pending);
        assert!(
            !diag[0].heuristics_active,
            "猶予中はまだ hook を待っている（推定は抑止されている）"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn diagnostics_marks_a_fallen_back_claude_session_as_active() {
        let (clock, _sink, reg) = setup(&[]);
        reg.register("s1", CliKind::Claude, true, 30);
        advance(&clock, 25_000).await;

        let diag = reg.diagnostics();
        assert_eq!(diag[0].liveness, HookLiveness::Unreachable);
        assert!(diag[0].heuristics_active, "不達なら推定が働いている");
    }

    /// **クランプの呼び出し側は 2 箇所ある**（`register` と `reconfigure`）。
    /// `timeout_secs_are_clamped_into_the_allowed_range` が測っているのは `register` 側
    /// だけで、`reconfigure` 側だけクランプを外す変異は全緑だった
    /// （実測: round m3-3-t9-rev-r1 の変異 S2 = 115/115）。純関数が守られていても
    /// 配線の片方が守られていない、という形（群 S）。
    ///
    /// 実害: `update_session` が `timeout_secs = 0` を渡すと `SessionActivity` の
    /// `timeout_ms == 0` になり、出力チャンクごとに立つウォッチャが必ず即 Fire する。
    /// チャンク 1 個につき `Silence` → `SilenceTimeout` が 1 件出て
    /// `Running` ↔ `Idle` が振動し続ける。
    ///
    /// ⚠️ `reconfigure` を **t=0 で呼ばない**のは意図的である。経過 0 ミリ秒だと
    /// `silence_step` が `elapsed <= 0` の早期 return で `Wait { ms: 0 }` を返し、
    /// クランプを外した実装が Fire ではなく `sleep(0)` のスピンに入る ——
    /// **赤ではなくハングになって変異が測れない。** 1 秒進めてから呼ぶこと。
    #[tokio::test(start_paused = true)]
    async fn reconfigure_clamps_the_timeout_into_the_allowed_range() {
        let (clock, sink, reg) = setup(&[("s1", RuntimeState::Running)]);
        let act = reg.register("s1", CliKind::Custom, true, 300);
        act.record_output(0);
        settle().await;

        advance(&clock, 1_000).await;
        reg.reconfigure("s1", true, 0); // 下限へ丸められる
        settle().await;
        assert!(
            sink.sent().is_empty(),
            "reconfigure が 0 をそのまま渡している（経過 1 秒で発火した）"
        );

        // 丸めた下限に満たないうちは発火しない
        advance(&clock, (MIN_SILENCE_TIMEOUT_SECS as u64) * 1_000 - 1_500).await;
        assert!(
            sink.sent().is_empty(),
            "丸めた下限より前に発火している（reconfigure 側のクランプが効いていない）"
        );

        advance(&clock, 1_000).await;
        assert_eq!(
            sink.sent(),
            vec![("s1".to_string(), StateInput::SilenceTimeout)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_secs_are_clamped_into_the_allowed_range() {
        let (clock, sink, reg) = setup(&[("s1", RuntimeState::Running)]);
        let act = reg.register("s1", CliKind::Custom, true, 0); // 下限へ丸められる
        act.record_output(0);
        tokio::task::yield_now().await;

        // 下限に満たないうちは発火しない（丸めずに 0 を通すと即発火してしまう）
        advance(&clock, (MIN_SILENCE_TIMEOUT_SECS as u64) * 1_000 - 500).await;
        assert!(
            sink.sent().is_empty(),
            "丸めた下限より前に発火している（クランプが効いていない）"
        );

        advance(&clock, 1_000).await;
        assert_eq!(sink.sent().len(), 1);
    }
}
