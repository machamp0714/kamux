//! セッションの runtime_state 状態機械。
//! 遷移判断はこのファイルの純粋関数 `next_state` に閉じ込める。

use crate::model::{RuntimeState, StateReason};
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// 状態機械への入力。
/// 契約 §8 の `StateReason`（13 バリアント）から `StartupNormalize` / `SpawnFailed` を除いた 11 個。
/// `StartupNormalize` を**含めない**ことが「`interrupted` を実行中に付けない」の型レベル強制になる。
/// `SpawnFailed` を含めない理由は §2.2.1（`error` は遷移表を経由しない）。
/// `ResumeFailed` を含める理由は契約 §41.3（`exited` + reason=ResumeFailed の唯一の発行手段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateInput {
    Spawned,
    OutputActivity,
    UserInput,
    HookNotification,
    HookPermission,
    HookStop,
    PtyExited,
    ResumeFailed,
    UserStopped,
    BelDetected,
    SilenceTimeout,
}

impl StateInput {
    pub const ALL: [StateInput; 11] = [
        StateInput::Spawned,
        StateInput::OutputActivity,
        StateInput::UserInput,
        StateInput::HookNotification,
        StateInput::HookPermission,
        StateInput::HookStop,
        StateInput::PtyExited,
        StateInput::ResumeFailed,
        StateInput::UserStopped,
        StateInput::BelDetected,
        StateInput::SilenceTimeout,
    ];

    pub fn reason(self) -> StateReason {
        match self {
            StateInput::Spawned => StateReason::Spawned,
            StateInput::OutputActivity => StateReason::OutputActivity,
            StateInput::UserInput => StateReason::UserInput,
            StateInput::HookNotification => StateReason::HookNotification,
            StateInput::HookPermission => StateReason::HookPermission,
            StateInput::HookStop => StateReason::HookStop,
            StateInput::PtyExited => StateReason::PtyExited,
            StateInput::ResumeFailed => StateReason::ResumeFailed,
            StateInput::UserStopped => StateReason::UserStopped,
            StateInput::BelDetected => StateReason::BelDetected,
            StateInput::SilenceTimeout => StateReason::SilenceTimeout,
        }
    }
}

/// 状態遷移の唯一の判断。副作用なし。
///
/// `None` は「遷移しない」を意味し、呼び出し側は DB 書き込みもイベント発火も行わない。
/// この設計により、PTY 出力チャンク毎に `OutputActivity` が届いても
/// すでに `running` なら何も起きない(契約 §0 のアイドル CPU 要件)。
///
/// 戻り値に `RuntimeState::Interrupted` は決して現れない(契約 §2)。
pub fn next_state(current: RuntimeState, input: StateInput) -> Option<(RuntimeState, StateReason)> {
    use RuntimeState::*;
    use StateInput as In;

    let target = match (current, input) {
        // Spawned はあらゆる状態(exited / interrupted を含む)から running へ戻す唯一の入力
        (_, In::Spawned) => Running,

        // 終了状態・error は Spawned 以外を一切受け付けない
        // (error 行の唯一の出口が Spawned であることは契約 §40.2 / §41.3)
        (Exited, _) | (Interrupted, _) | (Error, _) => return None,

        // waiting_input は「出力があった」「沈黙した」では解除しない。
        // Claude Code の TUI は入力待ちプロンプトでもスピナー等でバイトを吐くため、
        // ここを許すと Notification 直後に 🟡 が 🟢 へ戻り、通知の意味が失われる。
        (WaitingInput, In::OutputActivity) | (WaitingInput, In::SilenceTimeout) => return None,

        (_, In::OutputActivity) | (_, In::UserInput) => Running,
        // 契約 §12.4: PermissionRequest は「ユーザーの承認待ち」の最も直接的な信号。
        // Notification と両方登録し、どちらが来ても 🟡 へ遷移させる。
        (_, In::HookNotification) | (_, In::HookPermission) | (_, In::BelDetected) => WaitingInput,
        (_, In::HookStop) | (_, In::SilenceTimeout) => Idle,
        // ResumeFailed は PtyExited の逐語コピー。違うのは reason だけ(契約 §41.3)
        (_, In::PtyExited) | (_, In::ResumeFailed) | (_, In::UserStopped) => Exited,
    };

    if target == current {
        None
    } else {
        Some((target, input.reason()))
    }
}

/// 起動時の正規化。**`RuntimeState::Interrupted` を生成できる唯一の場所**（契約 §2）。
///
/// `None` は「据え置き（DB を書き換えない）」を意味する。
/// `idle` を据え置くのは、スキーマの DEFAULT が 'idle' であり、
/// 一度も起動していない Backlog のカードまで ⏸ にしないため。
///
/// 呼び出し元は `RuntimeStateManager::normalize_on_startup()` ただ1つ。
pub fn normalize_startup_state(last: RuntimeState) -> Option<RuntimeState> {
    match last {
        // アプリ終了時に PTY が死んでいる = 実行中のまま中断された
        RuntimeState::Running | RuntimeState::WaitingInput => Some(RuntimeState::Interrupted),
        // `Error` は据え置き（None）。契約 §40.5「❌ は再起動後も残らなければならない」。
        RuntimeState::Idle
        | RuntimeState::Exited
        | RuntimeState::Interrupted
        | RuntimeState::Error => None,
    }
}

/// surface_id の suffix（契約 §5）。agent サーフェスだけが状態機械に影響する。
const SURFACE_KIND_AGENT: &str = "agent";

pub type StateMap = HashMap<String, RuntimeState>;

/// ロック毒化で panic しないためのヘルパ（契約 §0: unwrap 禁止）。
/// 毒化は「別スレッドが保持中に panic した」だけで、マップ自体の不変条件は壊れていない。
pub(crate) fn read_map(m: &RwLock<StateMap>) -> RwLockReadGuard<'_, StateMap> {
    m.read().unwrap_or_else(|e| e.into_inner())
}

// Task 4 の `RuntimeStateManager`（次タスク）が状態遷移の書き込みに使う。
// このタスク単体では未使用のため dead_code を明示的に許可する。
#[expect(dead_code, reason = "Task 4 の RuntimeStateManager が消費する公開 API")]
pub(crate) fn write_map(m: &RwLock<StateMap>) -> RwLockWriteGuard<'_, StateMap> {
    m.write().unwrap_or_else(|e| e.into_inner())
}

/// チャネルを流れる 1 件の入力。
#[derive(Debug)]
pub struct RuntimeEvent {
    pub session_id: String,
    pub input: StateInput,
}

/// `mpsc::Sender` は `Send` だが **`!Sync`** である。
/// `AppState` が `Arc<RuntimeStateManager>` を保持し、Tauri の `State<'_, T>` が
/// `T: Send + Sync + 'static` を要求するため、`Mutex` で包んで `Sync` にする。
/// ロックを持つのはキューへの push の間だけで、ブロッキングは実質発生しない。
pub type EventTx = Arc<Mutex<Sender<RuntimeEvent>>>;

/// 状態機械への唯一の入口。各スレッドへ Clone して配る。
#[derive(Clone)]
pub struct RuntimeSender {
    states: Arc<RwLock<StateMap>>,
    tx: EventTx,
}

impl RuntimeSender {
    pub fn new(states: Arc<RwLock<StateMap>>, tx: EventTx) -> Self {
        Self { states, tx }
    }

    /// 現在の runtime_state。未知のセッションは `Idle`。
    pub fn current(&self, session_id: &str) -> RuntimeState {
        read_map(&self.states)
            .get(session_id)
            .copied()
            .unwrap_or(RuntimeState::Idle)
    }

    /// session_id を直接指定して入力を送る。M2-2（hooks）/ M3-3（ヒューリスティック）用。
    /// consumer 側が落ちている場合は黙って捨てる（アプリ終了時に起きうる正常系）。
    pub fn send(&self, session_id: &str, input: StateInput) {
        let event = RuntimeEvent {
            session_id: session_id.to_string(),
            input,
        };
        let tx = self.tx.lock().unwrap_or_else(|e| e.into_inner());
        let _ = tx.send(event);
    }

    /// surface_id 経由で送る。`sink.rs` と `write_pty` コマンド用。
    ///
    /// - `:editor` サーフェスは無視する(nvim を閉じてもセッションは ⛔ にならない)
    /// - 現在状態から遷移が起きない入力は、`String` の確保もチャネル送信もせず捨てる。
    ///   出力チャンク毎・キー入力毎に呼ばれるためのコスト対策(契約 §0)。
    ///   判定と consumer 処理の間に状態が変わるレースはありうるが、捨てられるのは
    ///   「送っても None になるはずだった入力」であり、後続の入力が正しい遷移を起こす。
    pub fn note_surface(&self, surface_id: &str, input: StateInput) {
        let Some((session_id, kind)) = surface_id.rsplit_once(':') else {
            return;
        };
        if kind != SURFACE_KIND_AGENT {
            return;
        }
        if next_state(self.current(session_id), input).is_none() {
            return;
        }
        self.send(session_id, input);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RuntimeState::*;
    use StateInput as In;

    const ALL_STATES: [RuntimeState; 6] = [Running, WaitingInput, Idle, Exited, Interrupted, Error];

    /// (現在状態, 入力, 遷移先 + reason の期待値)。`next_state` の戻り値の型と揃えている。
    type TableRow = (
        RuntimeState,
        StateInput,
        Option<(RuntimeState, StateReason)>,
    );

    /// 設計書 M2-1 §2.2 の遷移表そのもの。66 セルすべてを列挙する。
    /// None = 遷移なし（イベント発火なし・DB 書き込みなし）
    /// Some の第 2 要素（`StateReason`）は `next_state` の戻り値と丸ごと突き合わせる
    /// ためのリテラル。`StateInput::reason()` から導出しない
    /// （実装と同じ式で期待値を作ると、reason の取り違えを検出できない）。
    /// ResumeFailed 列は PtyExited 列の逐語コピー（契約 §41.3）。
    const TABLE: [TableRow; 66] = [
        // running
        (Running, In::Spawned, None),
        (Running, In::OutputActivity, None),
        (Running, In::UserInput, None),
        (
            Running,
            In::HookNotification,
            Some((WaitingInput, StateReason::HookNotification)),
        ),
        (
            Running,
            In::HookPermission,
            Some((WaitingInput, StateReason::HookPermission)),
        ),
        (Running, In::HookStop, Some((Idle, StateReason::HookStop))),
        (
            Running,
            In::PtyExited,
            Some((Exited, StateReason::PtyExited)),
        ),
        (
            Running,
            In::ResumeFailed,
            Some((Exited, StateReason::ResumeFailed)),
        ),
        (
            Running,
            In::UserStopped,
            Some((Exited, StateReason::UserStopped)),
        ),
        (
            Running,
            In::BelDetected,
            Some((WaitingInput, StateReason::BelDetected)),
        ),
        (
            Running,
            In::SilenceTimeout,
            Some((Idle, StateReason::SilenceTimeout)),
        ),
        // waiting_input
        (
            WaitingInput,
            In::Spawned,
            Some((Running, StateReason::Spawned)),
        ),
        (WaitingInput, In::OutputActivity, None),
        (
            WaitingInput,
            In::UserInput,
            Some((Running, StateReason::UserInput)),
        ),
        (WaitingInput, In::HookNotification, None),
        (WaitingInput, In::HookPermission, None),
        (
            WaitingInput,
            In::HookStop,
            Some((Idle, StateReason::HookStop)),
        ),
        (
            WaitingInput,
            In::PtyExited,
            Some((Exited, StateReason::PtyExited)),
        ),
        (
            WaitingInput,
            In::ResumeFailed,
            Some((Exited, StateReason::ResumeFailed)),
        ),
        (
            WaitingInput,
            In::UserStopped,
            Some((Exited, StateReason::UserStopped)),
        ),
        (WaitingInput, In::BelDetected, None),
        (WaitingInput, In::SilenceTimeout, None),
        // idle
        (Idle, In::Spawned, Some((Running, StateReason::Spawned))),
        (
            Idle,
            In::OutputActivity,
            Some((Running, StateReason::OutputActivity)),
        ),
        (Idle, In::UserInput, Some((Running, StateReason::UserInput))),
        (
            Idle,
            In::HookNotification,
            Some((WaitingInput, StateReason::HookNotification)),
        ),
        (
            Idle,
            In::HookPermission,
            Some((WaitingInput, StateReason::HookPermission)),
        ),
        (Idle, In::HookStop, None),
        (Idle, In::PtyExited, Some((Exited, StateReason::PtyExited))),
        (
            Idle,
            In::ResumeFailed,
            Some((Exited, StateReason::ResumeFailed)),
        ),
        (
            Idle,
            In::UserStopped,
            Some((Exited, StateReason::UserStopped)),
        ),
        (
            Idle,
            In::BelDetected,
            Some((WaitingInput, StateReason::BelDetected)),
        ),
        (Idle, In::SilenceTimeout, None),
        // exited
        (Exited, In::Spawned, Some((Running, StateReason::Spawned))),
        (Exited, In::OutputActivity, None),
        (Exited, In::UserInput, None),
        (Exited, In::HookNotification, None),
        (Exited, In::HookPermission, None),
        (Exited, In::HookStop, None),
        (Exited, In::PtyExited, None),
        (Exited, In::ResumeFailed, None),
        (Exited, In::UserStopped, None),
        (Exited, In::BelDetected, None),
        (Exited, In::SilenceTimeout, None),
        // interrupted
        (
            Interrupted,
            In::Spawned,
            Some((Running, StateReason::Spawned)),
        ),
        (Interrupted, In::OutputActivity, None),
        (Interrupted, In::UserInput, None),
        (Interrupted, In::HookNotification, None),
        (Interrupted, In::HookPermission, None),
        (Interrupted, In::HookStop, None),
        (Interrupted, In::PtyExited, None),
        (Interrupted, In::ResumeFailed, None),
        (Interrupted, In::UserStopped, None),
        (Interrupted, In::BelDetected, None),
        (Interrupted, In::SilenceTimeout, None),
        // error
        (Error, In::Spawned, Some((Running, StateReason::Spawned))),
        (Error, In::OutputActivity, None),
        (Error, In::UserInput, None),
        (Error, In::HookNotification, None),
        (Error, In::HookPermission, None),
        (Error, In::HookStop, None),
        (Error, In::PtyExited, None),
        (Error, In::ResumeFailed, None),
        (Error, In::UserStopped, None),
        (Error, In::BelDetected, None),
        (Error, In::SilenceTimeout, None),
    ];

    #[test]
    fn transition_table_is_complete() {
        assert_eq!(TABLE.len(), ALL_STATES.len() * StateInput::ALL.len());
        for state in ALL_STATES {
            for input in StateInput::ALL {
                let hits = TABLE
                    .iter()
                    .filter(|(s, i, _)| *s == state && *i == input)
                    .count();
                assert_eq!(hits, 1, "遷移表に {:?} x {:?} が {} 件", state, input, hits);
            }
        }
    }

    /// lane-controller 指定の「遷移あり 26 / 遷移なし 40」を数値として焼く。
    /// TABLE と実装を同時に書き換えるような変異（遷移する/しないの総数がずれる）を
    /// `next_state_matches_table` 単体より広く弁別する。
    #[test]
    fn transition_table_has_26_transitions_and_40_non_transitions() {
        let transitions = TABLE.iter().filter(|(_, _, next)| next.is_some()).count();
        let non_transitions = TABLE.iter().filter(|(_, _, next)| next.is_none()).count();
        assert_eq!(transitions, 26, "遷移ありセルは 26 件のはず");
        assert_eq!(non_transitions, 40, "遷移なしセルは 40 件のはず");
    }

    /// `next_state` の戻り値（遷移先 + reason）を TABLE の期待値と丸ごと突き合わせる。
    /// reason 側の期待値は TABLE にリテラルで書かれているため、
    /// `StateInput::reason()` 内で reason を取り違える変異もここで検出できる。
    #[test]
    fn next_state_matches_table() {
        for (current, input, expected) in TABLE {
            let actual = next_state(current, input);
            assert_eq!(actual, expected, "{:?} x {:?}", current, input);
        }
    }

    /// 契約 §2 の不変条件: `interrupted` は実行中の遷移では絶対に生成されない。
    #[test]
    fn next_state_never_yields_interrupted() {
        for state in ALL_STATES {
            for input in StateInput::ALL {
                if let Some((next, _)) = next_state(state, input) {
                    assert_ne!(
                        next, Interrupted,
                        "{:?} x {:?} が interrupted を生成した",
                        state, input
                    );
                }
            }
        }
    }

    /// 遷移するなら必ず現在状態と異なる（エッジトリガの保証）。
    #[test]
    fn transitions_always_change_state() {
        for state in ALL_STATES {
            for input in StateInput::ALL {
                if let Some((next, _)) = next_state(state, input) {
                    assert_ne!(next, state, "{:?} x {:?} が同一状態を返した", state, input);
                }
            }
        }
    }

    /// 終了状態からの唯一の出口は Spawned。
    #[test]
    fn terminal_states_only_exit_via_spawned() {
        for state in [Exited, Interrupted] {
            for input in StateInput::ALL {
                let result = next_state(state, input);
                if input == In::Spawned {
                    assert_eq!(result.map(|(s, _)| s), Some(Running));
                } else {
                    assert!(result.is_none(), "{:?} x {:?} が遷移した", state, input);
                }
            }
        }
    }

    /// 🟡 を出力活動で消さない（M2-3 の Dock バッジを守る）。
    #[test]
    fn waiting_input_is_not_cleared_by_output_or_silence() {
        assert!(next_state(WaitingInput, In::OutputActivity).is_none());
        assert!(next_state(WaitingInput, In::SilenceTimeout).is_none());
        assert_eq!(
            next_state(WaitingInput, In::UserInput).map(|(s, _)| s),
            Some(Running)
        );
    }

    /// 契約 §12.4: PermissionRequest は Notification と同じ 🟡 へ遷移する。
    /// 両方登録されるので、片方が来たあとにもう片方が来ても遷移しない（重複が無害）。
    #[test]
    fn hook_permission_behaves_exactly_like_hook_notification() {
        for state in ALL_STATES {
            assert_eq!(
                next_state(state, In::HookPermission).map(|(s, _)| s),
                next_state(state, In::HookNotification).map(|(s, _)| s),
                "{:?} で HookPermission と HookNotification の遷移先が違う",
                state
            );
        }
        // reason は区別できる（UI のツールチップ用）
        let (_, reason) = next_state(Running, In::HookPermission).expect("遷移するはず");
        assert_eq!(reason, StateReason::HookPermission);
    }

    /// `interrupted` を生成できる唯一の関数（契約 §2 / 提案 2）。
    #[test]
    fn normalize_promotes_only_live_states_to_interrupted() {
        assert_eq!(normalize_startup_state(Running), Some(Interrupted));
        assert_eq!(normalize_startup_state(WaitingInput), Some(Interrupted));
    }

    /// 一度も起動していない Backlog カード（DEFAULT 'idle'）を ⏸ にしてはいけない。
    #[test]
    fn normalize_leaves_resting_states_untouched() {
        assert_eq!(normalize_startup_state(Idle), None);
        assert_eq!(normalize_startup_state(Exited), None);
        // ⚠️ lane-controller 追加（2026-08-05）: 契約 §40.5
        //    「❌ は再起動後も残らなければならない」。原文にこの 1 行が無く、
        //    `Error` を ⏸ に化けさせる実装が緑で通ってしまう。
        assert_eq!(normalize_startup_state(Error), None);
    }

    /// 2 回続けて起動しても結果が変わらない。
    #[test]
    fn normalize_is_idempotent() {
        assert_eq!(normalize_startup_state(Interrupted), None);
        let once = normalize_startup_state(Running).expect("running は正規化される");
        assert_eq!(normalize_startup_state(once), None);
    }

    /// 正規化結果は interrupted か据え置きのみ。他の状態を捏造しない。
    #[test]
    fn normalize_only_ever_produces_interrupted() {
        for state in ALL_STATES {
            if let Some(next) = normalize_startup_state(state) {
                assert_eq!(
                    next, Interrupted,
                    "{:?} が interrupted 以外に正規化された",
                    state
                );
            }
        }
    }

    use std::sync::mpsc;
    use std::sync::{Arc, RwLock};

    fn sender_with(
        initial: &[(&str, RuntimeState)],
    ) -> (RuntimeSender, mpsc::Receiver<RuntimeEvent>) {
        let map: StateMap = initial
            .iter()
            .map(|(k, v)| ((*k).to_string(), *v))
            .collect();
        let states = Arc::new(RwLock::new(map));
        let (tx, rx) = mpsc::channel();
        (RuntimeSender::new(states, Arc::new(Mutex::new(tx))), rx)
    }

    /// `RuntimeEvent` は Task 11 で enum になる（契約 §40.2.1）。フィールドアクセスをここへ
    /// 集約し、enum 化時の書き換え面を 6 箇所から 1 箇所に減らす（lane-controller 指定）。
    fn expect_input(ev: RuntimeEvent) -> (String, StateInput) {
        (ev.session_id, ev.input)
    }

    /// `AppState` に `Arc<RuntimeStateManager>` を載せるため、Tauri の
    /// `State<'_, T>: Send + Sync + 'static` を満たす必要がある。
    /// `mpsc::Sender` は `Send` だが `!Sync` なので、`Mutex` で包んでいることを型で固定する。
    #[test]
    fn runtime_sender_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync + 'static>() {}
        assert_send_sync::<RuntimeSender>();
    }

    #[test]
    fn current_defaults_to_idle_for_unknown_session() {
        let (sender, _rx) = sender_with(&[]);
        assert_eq!(sender.current("nope"), Idle);
    }

    #[test]
    fn send_always_enqueues() {
        // In::OutputActivity は Running からは遷移しない(next_state は None)。
        // それでも送信されることが `send` と `note_surface` の違い
        // (note_surface だけが遷移なし入力を捨てる高速パスを持つ)。
        let (sender, rx) = sender_with(&[("s1", Running)]);
        sender.send("s1", In::OutputActivity);
        let ev = rx.try_recv().expect("送信されるはず");
        assert_eq!(expect_input(ev), ("s1".to_string(), In::OutputActivity));
    }

    #[test]
    fn note_surface_ignores_editor_surface() {
        let (sender, rx) = sender_with(&[("s1", Running)]);
        sender.note_surface("s1:editor", In::PtyExited);
        assert!(
            rx.try_recv().is_err(),
            "editor サーフェスは状態機械に届いてはいけない"
        );
    }

    #[test]
    fn note_surface_extracts_session_id_from_agent_surface() {
        let (sender, rx) = sender_with(&[("s1", Running)]);
        sender.note_surface("s1:agent", In::PtyExited);
        let ev = rx.try_recv().expect("送信されるはず");
        assert_eq!(expect_input(ev), ("s1".to_string(), In::PtyExited));
    }

    /// 出力チャンク毎・キー入力毎の allocation を避ける高速パス。
    #[test]
    fn note_surface_drops_inputs_that_would_not_transition() {
        let (sender, rx) = sender_with(&[("s1", Running)]);
        sender.note_surface("s1:agent", In::OutputActivity);
        sender.note_surface("s1:agent", In::UserInput);
        assert!(rx.try_recv().is_err(), "running 中の出力/入力は送信しない");
    }

    #[test]
    fn note_surface_sends_when_transition_would_happen() {
        let (sender, rx) = sender_with(&[("s1", WaitingInput)]);
        sender.note_surface("s1:agent", In::UserInput);
        let ev = rx.try_recv().expect("送信されるはず");
        assert_eq!(expect_input(ev).1, In::UserInput);
    }

    #[test]
    fn note_surface_ignores_malformed_surface_id() {
        let (sender, rx) = sender_with(&[("s1", Idle)]);
        sender.note_surface("no-colon-here", In::OutputActivity);
        assert!(rx.try_recv().is_err());
    }

    /// UUID にはハイフンが含まれるがコロンは含まれない。区切りは最後のコロン。
    #[test]
    fn note_surface_handles_uuid_session_id() {
        let uuid = "3f2a9c1e-0000-4000-8000-000000000001";
        let (sender, rx) = sender_with(&[(uuid, Idle)]);
        sender.note_surface(&format!("{}:agent", uuid), In::OutputActivity);
        let ev = rx.try_recv().expect("送信されるはず");
        assert_eq!(expect_input(ev).0, uuid);
    }
}
