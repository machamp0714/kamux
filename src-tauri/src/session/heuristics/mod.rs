//! 汎用 CLI 向けベストエフォート検知（設計書 §9.2 / §12）。
//!
//! hooks が使えない CLI、および hooks が届かない claude セッションに対して、
//! BEL 検知と沈黙タイムアウトから `RuntimeState` を推定する。
//! ここで導出される状態は必ず `gate::heuristic_transition` を通り、
//! hook 由来の権威ある遷移を上書きしない。

pub mod activity;
pub mod bel;
pub mod clock;
pub mod gate;
pub mod hook_liveness;
pub mod registry;
pub mod silence;
pub mod validate;

/// 沈黙タイムアウトの既定値（秒）。設計書 §9.2「既定 30 秒」
pub const DEFAULT_SILENCE_TIMEOUT_SECS: u32 = 30;
/// ユーザーが設定できる下限。0 を許すとウォッチャが busy loop になる。
/// **この下限を実際に強制しているのは `registry::clamp_timeout_secs` である** ——
/// `HeuristicRegistry::register` / `::reconfigure` が `SessionActivity` へ渡す
/// `silence_timeout_ms` は必ずそこを通る。**範囲検証（設定値を丸めずに弾く側）は
/// `validate::validate_silence_timeout_secs` が持つ**: クランプは黙って丸めるだけで、範囲外の入力を拒否しない。
/// （下記 const assert が固定するのは `DEFAULT_SILENCE_TIMEOUT_SECS` との
/// 大小順序だけで、0 を禁じる強制ではない。0 を禁じるのはクランプの側である。）
pub const MIN_SILENCE_TIMEOUT_SECS: u32 = 5;
/// ユーザーが設定できる上限（1 時間）
pub const MAX_SILENCE_TIMEOUT_SECS: u32 = 3600;
/// この窓の中で連続した BEL は 1 件に丸める（ms）
pub const BEL_DEBOUNCE_MS: i64 = 1_000;
/// claude セッションで hook を待つ猶予（ms）。これを過ぎたら hooks 不達と判定する。
/// `DEFAULT_SILENCE_TIMEOUT_SECS` より短いことが重要（設計 §4.7）
pub const HOOK_GRACE_MS: i64 = 20_000;

// `HOOK_GRACE_MS` は `DEFAULT_SILENCE_TIMEOUT_SECS` より短くなければならない
// （設計 §4.7）。この順序が崩れると「猶予切れ → 沈黙推定の発火」という
// hooks 不達判定の前提が壊れるため、既定値どうしのこの 1 本の関係はビルドで検知する。
const _: () = assert!(HOOK_GRACE_MS < DEFAULT_SILENCE_TIMEOUT_SECS as i64 * 1000);

// `DEFAULT_SILENCE_TIMEOUT_SECS` は常に `MIN_SILENCE_TIMEOUT_SECS..=MAX_SILENCE_TIMEOUT_SECS`
// の範囲に収まらなければならない。Task 11 の範囲検証はこの区間をユーザー入力の
// 許容範囲として使うため、既定値がこの区間の外に出ると既定値そのものが検証で
// 弾かれる。3 定数の大小順序をビルドで固定する（下限に 0 を禁じる等の値そのものの
// 妥当性は `registry::clamp_timeout_secs` の責務であり、この不等式が保証する範囲ではない）。
const _: () = assert!(MIN_SILENCE_TIMEOUT_SECS <= DEFAULT_SILENCE_TIMEOUT_SECS);
const _: () = assert!(DEFAULT_SILENCE_TIMEOUT_SECS <= MAX_SILENCE_TIMEOUT_SECS);

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use self::activity::SessionActivity;
use self::bel::BelScanner;
use crate::model::RuntimeState;
use crate::session::runtime_state::{next_state, StateInput};

/// M2-1 の状態機械への入力口。M3-3 はこの trait 越しにだけ状態機械を叩く。
/// 具象メソッド名に依存しないことで、M3-3 のロジックをフェイクで完結してテストできる。
pub trait RuntimeStateSink: Send + Sync + 'static {
    fn current(&self, session_id: &str) -> Option<RuntimeState>;
    /// 契約 §41.4 / M2-1 §5.1: 渡すのは**入力**であって次の状態ではない。
    fn send(&self, session_id: &str, input: StateInput);
}

/// PTY 読み取りスレッドが出力チャンクを通知する先（M1-3 との唯一の結合点）。
/// 読み取りスレッドが所有するので `&mut self` を取れる。ロックが不要になる。
pub trait OutputObserver: Send + 'static {
    fn on_chunk(&mut self, chunk: &[u8]);
}

/// ホットパスから消費側タスクへ渡すイベント。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeuristicEvent {
    Bel { session_id: String },
    Silence { session_id: String },
}

impl HeuristicEvent {
    pub fn session_id(&self) -> &str {
        match self {
            HeuristicEvent::Bel { session_id } | HeuristicEvent::Silence { session_id } => {
                session_id
            }
        }
    }

    pub fn input(&self) -> gate::HeuristicInput {
        match self {
            HeuristicEvent::Bel { .. } => gate::HeuristicInput::Bel,
            HeuristicEvent::Silence { .. } => gate::HeuristicInput::Silence,
        }
    }
}

/// agent サーフェスの読み取りスレッドが所有するオブザーバ。
/// `BelScanner` を所有するのでロックが要らない。
///
/// editor サーフェス（nvim）には**装着しない**。nvim は常時 BEL を鳴らし、
/// ユーザーが編集していない間は当然沈黙するため、混ぜると値が壊れる（設計 §4.8）。
/// 装着するかどうかを決めるのは `PtySurface::spawn` の呼び出し側であり、
/// `spawn` の中で `surface_id` を見て捨てる形にはしない（判断が 2 箇所に散る）。
pub struct AgentOutputObserver {
    activity: Arc<SessionActivity>,
    scanner: BelScanner,
}

impl AgentOutputObserver {
    pub fn new(activity: Arc<SessionActivity>) -> Self {
        Self {
            activity,
            scanner: BelScanner::new(),
        }
    }
}

impl OutputObserver for AgentOutputObserver {
    fn on_chunk(&mut self, chunk: &[u8]) {
        let bel_count = self.scanner.scan(chunk);
        self.activity.record_output(bel_count);
    }
}

/// テスト用の `RuntimeStateSink`。送られた入力を順に記録する。
#[derive(Debug, Default)]
pub struct FakeSink {
    inner: Mutex<FakeSinkInner>,
}

#[derive(Debug, Default)]
struct FakeSinkInner {
    states: HashMap<String, RuntimeState>,
    sent: Vec<(String, StateInput)>,
}

impl FakeSink {
    pub fn new(initial: &[(&str, RuntimeState)]) -> Self {
        let states = initial
            .iter()
            .map(|(id, st)| ((*id).to_string(), *st))
            .collect();
        Self {
            inner: Mutex::new(FakeSinkInner {
                states,
                sent: Vec::new(),
            }),
        }
    }

    /// 状態機械へ送られた入力の履歴。
    pub fn sent(&self) -> Vec<(String, StateInput)> {
        self.lock().sent.clone()
    }

    /// 毒された Mutex でも panic しない（契約 §0「unwrap() を使った panic 経路」の禁止）
    fn lock(&self) -> std::sync::MutexGuard<'_, FakeSinkInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl RuntimeStateSink for FakeSink {
    fn current(&self, session_id: &str) -> Option<RuntimeState> {
        self.lock().states.get(session_id).copied()
    }

    fn send(&self, session_id: &str, input: StateInput) {
        let mut guard = self.lock();
        guard.sent.push((session_id.to_string(), input));
        // 遷移表を写さず M2-1 の next_state を引く（契約 §41.4）。
        // 遷移が起きない入力は状態を動かさない —— 本番の consumer と同じ振る舞い。
        let current = guard
            .states
            .get(session_id)
            .copied()
            .unwrap_or(RuntimeState::Idle);
        if let Some((next, _reason)) = next_state(current, input) {
            guard.states.insert(session_id.to_string(), next);
        }
    }
}

#[cfg(test)]
mod observer_tests {
    use super::activity::SessionActivity;
    use super::clock::TestClock;
    use super::*;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    fn setup() -> (
        TestClock,
        Box<dyn OutputObserver>,
        mpsc::UnboundedReceiver<HeuristicEvent>,
    ) {
        let clock = TestClock::new(0);
        let (tx, rx) = mpsc::unbounded_channel();
        let activity = SessionActivity::new(
            "s1".to_string(),
            Arc::new(clock.clone()),
            tx,
            tokio::runtime::Handle::current(),
            true,
            30_000,
        );
        (clock, Box::new(AgentOutputObserver::new(activity)), rx)
    }

    #[tokio::test(start_paused = true)]
    async fn a_real_bel_in_a_chunk_produces_an_event() {
        let (_c, mut obs, mut rx) = setup();
        obs.on_chunk(b"waiting for input\x07");
        assert_eq!(
            rx.try_recv().unwrap(),
            HeuristicEvent::Bel {
                session_id: "s1".into()
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_osc_title_sequence_produces_no_event() {
        // シェルのプロンプトが毎回吐くシーケンス。ここで誤検知すると実用にならない
        let (_c, mut obs, mut rx) = setup();
        obs.on_chunk(b"\x1b]0;user@host: ~/repo\x07$ ");
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn scanner_state_survives_across_chunks() {
        let (_c, mut obs, mut rx) = setup();
        obs.on_chunk(b"\x1b]0;split-");
        obs.on_chunk(b"title\x07");
        assert!(
            rx.try_recv().is_err(),
            "チャンク跨ぎの OSC 終端子を誤検知した"
        );

        obs.on_chunk(b"\x07");
        assert_eq!(
            rx.try_recv().unwrap(),
            HeuristicEvent::Bel {
                session_id: "s1".into()
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn plain_output_starts_the_silence_watcher() {
        let (clock, mut obs, mut rx) = setup();
        obs.on_chunk(b"building...\n");
        tokio::task::yield_now().await;

        clock.advance_ms(31_000);
        tokio::time::advance(std::time::Duration::from_millis(31_000)).await;
        tokio::task::yield_now().await;

        assert_eq!(
            rx.try_recv().unwrap(),
            HeuristicEvent::Silence {
                session_id: "s1".into()
            }
        );
    }
}

#[cfg(test)]
mod sink_tests {
    use super::*;
    use crate::model::RuntimeState;
    use crate::session::runtime_state::StateInput;

    #[test]
    fn fake_sink_reports_initial_state() {
        let sink = FakeSink::new(&[
            ("s1", RuntimeState::Running),
            ("s2", RuntimeState::WaitingInput),
        ]);
        assert_eq!(sink.current("s1"), Some(RuntimeState::Running));
        assert_eq!(sink.current("s2"), Some(RuntimeState::WaitingInput));
        assert_eq!(sink.current("unknown"), None);
    }

    #[test]
    fn fake_sink_records_sent_inputs_in_order() {
        let sink = FakeSink::new(&[("s1", RuntimeState::Running)]);
        sink.send("s1", StateInput::SilenceTimeout);
        sink.send("s1", StateInput::BelDetected);
        assert_eq!(
            sink.sent(),
            vec![
                ("s1".to_string(), StateInput::SilenceTimeout),
                ("s1".to_string(), StateInput::BelDetected),
            ]
        );
    }

    /// FakeSink は遷移表を写さず `next_state` を引く（契約 §41.4）。
    #[test]
    fn fake_sink_send_advances_current_through_the_transition_table() {
        let sink = FakeSink::new(&[("s1", RuntimeState::Running)]);
        sink.send("s1", StateInput::SilenceTimeout);
        assert_eq!(sink.current("s1"), Some(RuntimeState::Idle));
    }

    #[test]
    fn fake_sink_is_usable_as_dyn_runtime_state_sink() {
        let sink: std::sync::Arc<dyn RuntimeStateSink> =
            std::sync::Arc::new(FakeSink::new(&[("s1", RuntimeState::Running)]));
        sink.send("s1", StateInput::SilenceTimeout);
        assert_eq!(sink.current("s1"), Some(RuntimeState::Idle));
    }

    /// `send` は `session_id` をキーとして正しく振り分ける。
    /// 片方のセッションへの `send` がもう片方の状態・履歴を動かしてはならない。
    #[test]
    fn fake_sink_send_keeps_sessions_independent() {
        let sink = FakeSink::new(&[("s1", RuntimeState::Running), ("s2", RuntimeState::Running)]);
        sink.send("s1", StateInput::SilenceTimeout);
        sink.send("s2", StateInput::BelDetected);
        assert_eq!(sink.current("s1"), Some(RuntimeState::Idle));
        assert_eq!(sink.current("s2"), Some(RuntimeState::WaitingInput));
        assert_eq!(
            sink.sent(),
            vec![
                ("s1".to_string(), StateInput::SilenceTimeout),
                ("s2".to_string(), StateInput::BelDetected),
            ]
        );
    }

    #[test]
    fn heuristic_event_carries_the_session_id() {
        let bel = HeuristicEvent::Bel {
            session_id: "s1".into(),
        };
        let silence = HeuristicEvent::Silence {
            session_id: "s2".into(),
        };
        assert_eq!(bel.session_id(), "s1");
        assert_eq!(silence.session_id(), "s2");
        assert_eq!(bel.input(), gate::HeuristicInput::Bel);
        assert_eq!(silence.input(), gate::HeuristicInput::Silence);
    }
}
