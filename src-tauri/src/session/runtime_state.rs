//! セッションの runtime_state 状態機械。
//! 遷移判断はこのファイルの純粋関数 `next_state` に閉じ込める。

use crate::error::AppResult;
use crate::model::{RuntimeState, SessionStatePayload, StateReason};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
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

/// `last_runtime_state` の永続化の抽象。本番は `Store`(契約 §17)、テストはフェイクを差す。
/// メソッド名は契約 §17 の `Store` API に揃える。
pub trait StatePersist: Send + Sync {
    fn set_last_runtime_state(&self, session_id: &str, state: RuntimeState) -> AppResult<()>;
    /// 指定した `last_runtime_state` を持つセッションの id を返す。起動時正規化用。
    fn list_ids_by_last_runtime_state(&self, state: RuntimeState) -> AppResult<Vec<String>>;
    /// 最初の PTY spawn 成功を 1 度だけ記録する(契約 §34.5)。
    /// **時計を引数に取らない** —— 状態機械を時計から切り離すため、`now_ms()` の呼び出しは
    /// `Store` アダプタ側に閉じる。冪等性は DAO の SQL が担保するので、
    /// 呼び出し側に「初回か」の判定を持たせないこと。
    fn mark_first_started(&self, session_id: &str) -> AppResult<()>;
    /// `error` への遷移を永続化する(契約 §38.8)。`mark_error` の唯一の永続化口。
    /// `last_runtime_state = 'error'` と `last_runtime_error` を DAO が 1 文で書く。
    /// **`set_last_runtime_state(id, Error)` は使わない** —— メッセージの無い error 行ができる
    fn set_runtime_error(&self, session_id: &str, message: &str) -> AppResult<()>;
}

/// 遷移が確定したあとに呼ばれる。Tauri emit と M2-3 の通知がこれを実装する。
/// consumer スレッドから同期的に呼ばれるので、重い処理をここでしないこと。
pub trait StateObserver: Send + Sync {
    fn on_state(&self, payload: &SessionStatePayload);
}

type Observers = Arc<RwLock<Vec<Arc<dyn StateObserver>>>>;

/// runtime_state の直列化層。
/// **DB への `last_runtime_state` 書き込みは、この型の consumer スレッドだけが行う。**
pub struct RuntimeStateManager {
    states: Arc<RwLock<StateMap>>,
    tx: EventTx,
    persist: Arc<dyn StatePersist>,
    observers: Observers,
    shutting_down: Arc<AtomicBool>,
}

impl RuntimeStateManager {
    /// consumer スレッドを 1 本起動する。`recv()` でブロックするのでポーリングは発生しない。
    pub fn new(persist: Arc<dyn StatePersist>) -> Arc<Self> {
        let (tx, rx) = mpsc::channel::<RuntimeEvent>();
        let mgr = Arc::new(RuntimeStateManager {
            states: Arc::new(RwLock::new(StateMap::new())),
            tx: Arc::new(Mutex::new(tx)),
            persist,
            observers: Arc::new(RwLock::new(Vec::new())),
            shutting_down: Arc::new(AtomicBool::new(false)),
        });

        let states = Arc::clone(&mgr.states);
        let persist = Arc::clone(&mgr.persist);
        let observers = Arc::clone(&mgr.observers);
        let shutting_down = Arc::clone(&mgr.shutting_down);

        if let Err(err) = std::thread::Builder::new()
            .name("kamux-runtime-state".into())
            .spawn(move || {
                consume_loop(rx, states, persist, observers, shutting_down);
            })
        {
            // ここが失敗すると runtime_state が一切更新されなくなる。黙って落とさない。
            eprintln!("[kamux] failed to spawn runtime-state thread: {}", err);
        }

        mgr
    }

    pub fn sender(&self) -> RuntimeSender {
        RuntimeSender::new(Arc::clone(&self.states), Arc::clone(&self.tx))
    }

    pub fn register_observer(&self, observer: Arc<dyn StateObserver>) {
        self.observers
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .push(observer);
    }

    pub fn current(&self, session_id: &str) -> RuntimeState {
        read_map(&self.states)
            .get(session_id)
            .copied()
            .unwrap_or(RuntimeState::Idle)
    }

    /// アプリ終了の開始を宣言する。以降、メモリ・DB・observer のいずれも更新しない。
    /// PTY teardown より前に呼ぶこと(そうしないと `exited` が DB に残り、
    /// 次回起動時に ⏸ ではなく ⛔ が表示される)。冪等。
    pub fn begin_shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
    }
}

fn consume_loop(
    rx: mpsc::Receiver<RuntimeEvent>,
    states: Arc<RwLock<StateMap>>,
    persist: Arc<dyn StatePersist>,
    observers: Observers,
    shutting_down: Arc<AtomicBool>,
) {
    while let Ok(ev) = rx.recv() {
        if shutting_down.load(Ordering::SeqCst) {
            continue;
        }

        // 契約 §34.5: Spawned を「観測した時点」で記録する。**遷移表を引く前に置くこと。**
        // running + Spawned は遷移なし(None)で continue するため、
        // 遷移後に置くとこの経路が黙って抜ける。
        // 冪等性は DAO 側の SQL(WHERE first_started_at IS NULL)が担保する。
        // SurfaceKind::Agent 限定は Spawned の発行元(start_session / resume_session)が
        // すでに構造的に保証しているので、ここで surface を再チェックしない(§34.5)。
        if ev.input == StateInput::Spawned {
            if let Err(err) = persist.mark_first_started(&ev.session_id) {
                eprintln!(
                    "[kamux] failed to record first_started_at for {}: {}",
                    ev.session_id, err
                );
            }
        }

        let current = read_map(&states)
            .get(&ev.session_id)
            .copied()
            .unwrap_or(RuntimeState::Idle);

        let Some((next, reason)) = next_state(current, ev.input) else {
            continue;
        };

        // 判定から書き込みまでの間に終了が始まっていたら、ここで捨てる(最後のゲート)。
        // メモリ・DB・observer のいずれも更新しない(計画 §4.4 の逐語)。
        if shutting_down.load(Ordering::SeqCst) {
            continue;
        }

        write_map(&states).insert(ev.session_id.clone(), next);

        if let Err(err) = persist.set_last_runtime_state(&ev.session_id, next) {
            // DB 書き込み失敗で状態機械を止めない。真実はメモリ側にある。
            eprintln!(
                "[kamux] failed to persist runtime_state for {}: {}",
                ev.session_id, err
            );
        }

        let payload = SessionStatePayload {
            session_id: ev.session_id,
            runtime_state: next,
            reason,
        };
        for observer in observers.read().unwrap_or_else(|e| e.into_inner()).iter() {
            observer.on_state(&payload);
        }
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

    use crate::model::SessionStatePayload;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    #[derive(Default)]
    struct FakePersist {
        rows: Mutex<Vec<(String, RuntimeState)>>,
        writes: Mutex<Vec<(String, RuntimeState)>>,
        first_started: Mutex<Vec<String>>,
        errors: Mutex<Vec<(String, String)>>, // 契約 §38.8 の set_runtime_error
        fail: AtomicBool,
        /// `mark_first_started` が呼ばれた**回数**(dedup 前)。`first_started` は
        /// DAO の冪等性を模して重複を積まないので、「Spawned を遷移表より前で
        /// 拾っているか」を回数側でも固定する(契約 §34.5)。
        first_started_calls: Mutex<usize>,
        /// `mark_first_started` の呼び出し時点で走らせるフック。
        /// consume_loop の「先頭ゲート通過後・DB 書き込み直前ゲートより前」という
        /// レース窓を決定的に再現するための注入点(変異検証専用)。
        on_mark_first_started: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    }

    impl FakePersist {
        fn with_rows(rows: &[(&str, RuntimeState)]) -> Arc<Self> {
            Arc::new(FakePersist {
                rows: Mutex::new(rows.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()),
                ..Default::default()
            })
        }
        fn writes(&self) -> Vec<(String, RuntimeState)> {
            self.writes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
        fn first_started(&self) -> Vec<String> {
            self.first_started
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
        fn first_started_calls(&self) -> usize {
            *self
                .first_started_calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
        }
        fn set_on_mark_first_started(&self, f: impl Fn() + Send + Sync + 'static) {
            *self
                .on_mark_first_started
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
        }
    }

    impl StatePersist for FakePersist {
        fn set_last_runtime_state(
            &self,
            session_id: &str,
            state: RuntimeState,
        ) -> crate::error::AppResult<()> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(crate::error::AppError::Db("boom".into()));
            }
            self.writes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((session_id.to_string(), state));
            Ok(())
        }

        fn mark_first_started(&self, session_id: &str) -> crate::error::AppResult<()> {
            *self
                .first_started_calls
                .lock()
                .unwrap_or_else(|e| e.into_inner()) += 1;
            if let Some(hook) = self
                .on_mark_first_started
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
            {
                hook();
            }
            let mut marks = self.first_started.lock().unwrap_or_else(|e| e.into_inner());
            if !marks.contains(&session_id.to_string()) {
                marks.push(session_id.to_string());
            }
            Ok(())
        }

        /// 契約 §38.8。`errors` は `Mutex<Vec<(String, String)>>`（`FakePersist` のフィールド）
        fn set_runtime_error(
            &self,
            session_id: &str,
            message: &str,
        ) -> crate::error::AppResult<()> {
            self.errors
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((session_id.to_string(), message.to_string()));
            Ok(())
        }

        fn list_ids_by_last_runtime_state(
            &self,
            state: RuntimeState,
        ) -> crate::error::AppResult<Vec<String>> {
            Ok(self
                .rows
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .filter(|(_, s)| *s == state)
                .map(|(id, _)| id.clone())
                .collect())
        }
    }

    #[derive(Default)]
    struct RecordingObserver {
        seen: Mutex<Vec<(String, RuntimeState, StateReason)>>,
    }

    impl RecordingObserver {
        fn seen(&self) -> Vec<(String, RuntimeState, StateReason)> {
            self.seen.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }

    impl StateObserver for RecordingObserver {
        fn on_state(&self, payload: &SessionStatePayload) {
            self.seen.lock().unwrap_or_else(|e| e.into_inner()).push((
                payload.session_id.clone(),
                payload.runtime_state,
                payload.reason,
            ));
        }
    }

    /// consumer スレッドは非同期なので、条件が満たされるまで短時間待つ。
    fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        cond()
    }

    #[test]
    fn manager_applies_transition_persists_and_notifies() {
        let persist = FakePersist::with_rows(&[]);
        let mgr = RuntimeStateManager::new(persist.clone());
        let obs = Arc::new(RecordingObserver::default());
        mgr.register_observer(obs.clone());

        mgr.sender().send("s1", In::Spawned);

        // 副作用の順は memory -> DB -> observer。最後に起きる observer 通知を待ってから
        // 先行する副作用を assert する(先行分だけ待って後続を assert すると、
        // consumer が preempt されたときに flake する)。
        assert!(wait_until(|| {
            obs.seen() == vec![("s1".to_string(), Running, StateReason::Spawned)]
        }));
        assert_eq!(mgr.current("s1"), Running);
        assert_eq!(persist.writes(), vec![("s1".to_string(), Running)]);
    }

    /// 契約 §34.5: Spawned は「観測した時点」で記録する。
    /// running + Spawned は遷移しない(next_state が None)ので、
    /// 記録を遷移の後ろに置くとこの 2 回目が抜ける —— そこを固定する。
    #[test]
    fn spawned_marks_first_started_even_without_a_transition() {
        let persist = FakePersist::with_rows(&[]);
        let mgr = RuntimeStateManager::new(persist.clone());

        mgr.sender().send("s1", In::Spawned);
        assert!(wait_until(|| mgr.current("s1") == Running));
        assert!(wait_until(
            || persist.first_started() == vec!["s1".to_string()]
        ));

        // running 中の 2 回目の Spawned は遷移しないが、記録の呼び出し自体は届く
        mgr.sender().send("s1", In::Spawned);
        mgr.sender().send("s1", In::HookStop);
        assert!(wait_until(|| mgr.current("s1") == Idle));
        // mark_first_started はメモリ更新より前に呼ばれるので、Idle 観測時点で
        // 2 回目の呼び出しは処理済み(レースしない)。dedup 前の呼び出し回数で、
        // 遷移が無くても呼ばれていることを固定する(§34.5「遷移表を引く前」)。
        assert_eq!(persist.first_started_calls(), 2);
        // DAO 側が冪等なので、フェイクも重複を積まない
        assert_eq!(persist.first_started(), vec!["s1".to_string()]);

        // Spawned 以外では記録しない
        mgr.sender().send("s2", In::UserInput);
        assert!(wait_until(|| mgr.current("s2") == Running));
        assert_eq!(persist.first_started(), vec!["s1".to_string()]);
    }

    #[test]
    fn manager_ignores_inputs_that_do_not_transition() {
        let persist = FakePersist::with_rows(&[]);
        let mgr = RuntimeStateManager::new(persist.clone());
        let obs = Arc::new(RecordingObserver::default());
        mgr.register_observer(obs.clone());

        mgr.sender().send("s1", In::Spawned);
        assert!(wait_until(|| mgr.current("s1") == Running));
        // running 中の OutputActivity は遷移しない → 書き込みも配信も増えない
        mgr.sender().send("s1", In::OutputActivity);
        mgr.sender().send("s1", In::HookStop);

        // 最後に起きる observer 通知を待ってから、先行する副作用を assert する。
        assert!(wait_until(|| obs.seen().len() == 2));
        assert_eq!(mgr.current("s1"), Idle);
        assert_eq!(persist.writes().len(), 2);
    }

    /// 計画 §4.4: 「判定から書き込みまでの間に終了が始まった」レース窓を決定的に再現する。
    /// `mark_first_started` のコールバックから `begin_shutdown()` を呼ぶことで、
    /// 先頭ゲートは通過済みだが DB 書き込み直前のゲートより前、という窓を毎回同じ場所で作る。
    /// メモリ・DB・observer のいずれも更新されないことを固定する(§4.4 の逐語)。
    #[test]
    fn shutdown_started_mid_event_updates_nothing() {
        let persist = FakePersist::with_rows(&[]);
        let mgr = RuntimeStateManager::new(persist.clone());
        let obs = Arc::new(RecordingObserver::default());
        mgr.register_observer(obs.clone());

        let mgr_for_hook = Arc::clone(&mgr);
        persist.set_on_mark_first_started(move || mgr_for_hook.begin_shutdown());

        mgr.sender().send("s1", In::Spawned);

        // フックが走った = 先頭ゲートを通過し、レース窓(第二ゲートの手前)に入った陽性の証拠。
        // 固定タイマーではなく、consumer が実際にここまで到達したことを待って確認する。
        assert!(wait_until(
            || persist.first_started() == vec!["s1".to_string()]
        ));

        assert!(
            persist.writes().is_empty(),
            "レース窓で始まった終了は DB へ書いてはいけない"
        );
        assert!(
            obs.seen().is_empty(),
            "レース窓で始まった終了は observer を呼んではいけない"
        );
        assert_eq!(
            mgr.current("s1"),
            Idle,
            "レース窓で始まった終了はメモリも更新してはいけない"
        );
    }

    /// 正常終了(Cmd+Q)で PTY kill による exited を DB に書かせない。
    /// DB に running を残すことが、次回起動時の ⏸ の根拠になる。
    #[test]
    fn begin_shutdown_stops_persisting_and_notifying() {
        let persist = FakePersist::with_rows(&[]);
        let mgr = RuntimeStateManager::new(persist.clone());
        let obs = Arc::new(RecordingObserver::default());
        mgr.register_observer(obs.clone());

        mgr.sender().send("s1", In::Spawned);
        assert!(wait_until(|| mgr.current("s1") == Running));

        mgr.begin_shutdown();
        mgr.sender().send("s1", In::PtyExited);

        // consumer が処理する猶予を与えたうえで、何も増えていないことを確かめる
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(persist.writes(), vec![("s1".to_string(), Running)]);
        assert_eq!(obs.seen().len(), 1);
        assert_eq!(mgr.current("s1"), Running);
    }

    #[test]
    fn begin_shutdown_is_idempotent() {
        let persist = FakePersist::with_rows(&[]);
        let mgr = RuntimeStateManager::new(persist.clone());
        mgr.begin_shutdown();
        mgr.begin_shutdown();
        mgr.sender().send("s1", In::Spawned);

        std::thread::sleep(Duration::from_millis(50));
        assert!(persist.writes().is_empty());
        assert!(persist.first_started().is_empty());
        assert_eq!(mgr.current("s1"), Idle);
    }

    /// DB 書き込みが失敗しても consumer スレッドは死なない(契約 §0: panic 経路禁止)。
    #[test]
    fn persist_failure_does_not_kill_consumer() {
        let persist = FakePersist::with_rows(&[]);
        persist.fail.store(true, Ordering::SeqCst);
        let mgr = RuntimeStateManager::new(persist.clone());
        let obs = Arc::new(RecordingObserver::default());
        mgr.register_observer(obs.clone());

        mgr.sender().send("s1", In::Spawned);
        // 1 回目の書き込みは fail=true で失敗するが、observer(:351) は
        // 失敗をログしたあとも呼ばれる(最後の副作用)。ここを待ってから
        // fail を倒す。current だけを待つと、consumer が DB 書き込みの
        // 失敗ログ処理中に fail の反転が間に合い、書き込みが成功して
        // しまうレースが起こりうる。
        assert!(wait_until(|| obs.seen().len() == 1));
        assert_eq!(mgr.current("s1"), Running);

        persist.fail.store(false, Ordering::SeqCst);
        mgr.sender().send("s1", In::HookStop);
        assert!(wait_until(|| obs.seen().len() == 2));
        assert_eq!(mgr.current("s1"), Idle);
        assert_eq!(persist.writes(), vec![("s1".to_string(), Idle)]);
    }

    /// sender 経由の note_surface が manager のスナップショットを見ていること。
    #[test]
    fn sender_shares_snapshot_with_manager() {
        let persist = FakePersist::with_rows(&[]);
        let mgr = RuntimeStateManager::new(persist.clone());
        let sender = mgr.sender();

        sender.send("s1", In::Spawned);
        // observer を assert しないこの経路では DB 書き込みが最後の副作用。それを待つ。
        assert!(wait_until(
            || persist.writes() == vec![("s1".to_string(), Running)]
        ));
        // このテストの本旨: sender 経由の読み取りが manager の共有スナップショットを
        // 見ていること。write_map(:336) は persist(:338) より前に確定するので、
        // writes() が観測できた時点でメモリも安定している。
        assert_eq!(sender.current("s1"), Running);

        // running 中の出力は捨てられる
        sender.note_surface("s1:agent", In::OutputActivity);
        assert_eq!(persist.writes(), vec![("s1".to_string(), Running)]);
    }

    /// `AppState` に `Arc<RuntimeStateManager>` を載せるため、Tauri の
    /// `State<'_, T>: Send + Sync + 'static` を満たす必要がある。
    #[test]
    fn runtime_state_manager_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync + 'static>() {}
        assert_send_sync::<RuntimeStateManager>();
    }
}
