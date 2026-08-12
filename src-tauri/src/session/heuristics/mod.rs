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
pub mod silence;

/// 沈黙タイムアウトの既定値（秒）。設計書 §9.2「既定 30 秒」
pub const DEFAULT_SILENCE_TIMEOUT_SECS: u32 = 30;
/// ユーザーが設定できる下限。0 を許すとウォッチャが busy loop になるため、
/// クランプ・範囲検証（Task 9 / Task 11）でこの下限を強制する予定である。
/// **現時点でこの定数を読む実装は無い**（下記 const assert が固定するのは
/// `DEFAULT_SILENCE_TIMEOUT_SECS` との大小順序だけで、0 を禁じる強制ではない）。
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
// 妥当性はクランプ〔Task 9〕の責務であり、この不等式が保証する範囲ではない）。
const _: () = assert!(MIN_SILENCE_TIMEOUT_SECS <= DEFAULT_SILENCE_TIMEOUT_SECS);
const _: () = assert!(DEFAULT_SILENCE_TIMEOUT_SECS <= MAX_SILENCE_TIMEOUT_SECS);

use std::collections::HashMap;
use std::sync::Mutex;

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
