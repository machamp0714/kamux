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

/// チャネルを流れる 1 件。
///
/// `Error` は**遷移表を引かない帯域外イベント**である（契約 §40.2.1）。
/// `StateInput` に `SpawnFailed` を足す形は禁止 —— 遷移表の全セルに
/// 「どこからでも error へ飛べる」経路が生まれ、`exited` / `interrupted` からの
/// 不正遷移を型で禁じられなくなる（契約 §2.2.1 / §40.8）。
#[derive(Debug)]
pub enum RuntimeEvent {
    Input {
        session_id: String,
        input: StateInput,
    },
    Error {
        session_id: String,
        message: String,
    },
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
        let event = RuntimeEvent::Input {
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

    /// PTY 終了の通知。**`sink.rs` の `on_exit` から呼ぶ唯一の入口。**
    /// `:editor` サーフェスは無視される（nvim を閉じてもセッションは ⛔ にならない。契約 §2）。
    ///
    /// フィルタは `note_surface` の内部に閉じたままにすること。呼び出し側にも
    /// 同じ判定を書くと 2 箇所に散り、片方だけ直した瞬間に「nvim を閉じただけで
    /// セッションが ⛔ になる」事故が戻ってくる。
    pub fn note_pty_exit(&self, surface_id: &str) {
        self.note_surface(surface_id, StateInput::PtyExited);
    }

    /// resume 失敗と分類された PTY 終了の通知（契約 §40.4 / §41.3）。
    /// **`note_pty_exit` とまったく同じ分解・同じ `:agent` フィルタを通る。**
    /// 分類は M2-4 の `ResumeTracker::classify_exit(surface_id, exit_code)` が行い、
    /// `sink.rs` はその結果でこの 2 本を出し分ける。遷移先は `PtyExited` と同一の
    /// `exited` で、違うのは `StateReason` だけである。
    ///
    /// **M2-1 の時点では呼び出し元が無い**（`ResumeTracker` は M2-4 で入る）。
    /// 契約 §44.1 により、呼び出し側が無いことを欠陥として扱わない。
    pub fn note_resume_failed_exit(&self, surface_id: &str) {
        self.note_surface(surface_id, StateInput::ResumeFailed);
    }

    /// ユーザーのキー入力の通知。**`write_pty` コマンドから呼ぶ唯一の入口。**
    /// 🟡 を解除できる唯一の経路であり、`running` 中の打鍵は送信前に捨てられる。
    pub fn note_user_input(&self, surface_id: &str) {
        self.note_surface(surface_id, StateInput::UserInput);
    }

    /// `error` ❌ の唯一の生成源（契約 §2 / §40）。**遷移表を経由しない。**
    ///
    /// 呼んでよいのは `start_session` / `resume_session` の**起動フェーズ**の Err だけで、
    /// 許可される Err は契約 §40.3 の表（`start_session` は §63.5 が 5 段に更新済み）が
    /// すべてである。事前条件エラー（二重起動ガードの `InvalidState` など）で呼ぶと、
    /// `running` の上に `error` が書かれ、`error` 行は `Spawned` しか受け付けず、
    /// ガードは PTY が生きている限り `InvalidState` を返し続けるので `Spawned` は
    /// 永遠に来ない —— **カードが ❌ のまま永久に固着する**（契約 §40.3）。
    /// **`spawn_editor` からは呼ばない**（契約 §40.3 / §40.8。nvim が開けないだけで
    /// エージェントは生きている。M3-1 の担当箇所）。
    ///
    /// `message` は加工しない生 stderr（`AppError` の `Display`）を渡すこと（契約 §2 / §6）。
    /// 戻り値を持たない —— 状態の確定も永続化も consumer スレッドが行い、DB 書き込みの
    /// 失敗はログに落ちる（契約 §40.2）。呼び出し元スレッドから `states` / `persist` を
    /// 直接書くと「DB 書き込みは consumer スレッドだけ」の単一書き手の規律が壊れ、
    /// consumer の read→write の窓に割り込んだ `error` が上書きされうる（契約 §40.2.1）。
    pub fn mark_error(&self, session_id: &str, message: &str) {
        let event = RuntimeEvent::Error {
            session_id: session_id.to_string(),
            message: message.to_string(),
        };
        let tx = self.tx.lock().unwrap_or_else(|e| e.into_inner());
        let _ = tx.send(event);
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

    /// 起動時正規化。**`interrupted` を DB に書き込む唯一の場所**（契約 §2）。
    ///
    /// - `running` / `waiting_input` の行を `interrupted` に更新する
    /// - すべての行でメモリ上のスナップショットを埋める（正規化されなかった行も含む）
    /// - イベントは emit しない。この時点で WebView はまだ listen していないため。
    ///   フロントは `list_sessions` の `last_runtime_state`（正規化済み）からシードする
    ///
    /// 全行を読み切ってから書き込む。読みながら書くと、interrupted に更新した行を
    /// 同じ走査の後半（Interrupted のパス）で読み直してしまう。
    ///
    /// 戻り値は実際に変更した (session_id, 新しい状態) の一覧。
    pub fn normalize_on_startup(&self) -> AppResult<Vec<(String, RuntimeState)>> {
        // RuntimeState は 6 値である(契約 §2)。Error を落とすと**コンパイルは通るが**
        // last_runtime_state='error' の行がメモリスナップショットに載らず、current(id) が
        // Idle を返す。次の入力が §2.2 の error 行(Spawned のみ許可)ではなく idle 行で
        // 評価され、❌ が HookNotification 1 発で 🟡 へ復帰する。フロントは list_sessions
        // から ❌ を seed するので、Rust 側だけが食い違う split-brain になる。
        const ALL_STATES: [RuntimeState; 6] = [
            RuntimeState::Running,
            RuntimeState::WaitingInput,
            RuntimeState::Idle,
            RuntimeState::Exited,
            RuntimeState::Interrupted,
            RuntimeState::Error,
        ];

        // 1) 先に全行を読む
        let mut rows: Vec<(String, RuntimeState)> = Vec::new();
        for last in ALL_STATES {
            for id in self.persist.list_ids_by_last_runtime_state(last)? {
                rows.push((id, last));
            }
        }

        // 2) そのあとで書く
        let mut changed = Vec::new();
        let mut map = write_map(&self.states);

        for (id, last) in rows {
            let effective = match normalize_startup_state(last) {
                Some(next) => {
                    self.persist.set_last_runtime_state(&id, next)?;
                    changed.push((id.clone(), next));
                    next
                }
                None => last,
            };
            map.insert(id, effective);
        }

        Ok(changed)
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

        let (session_id, next, reason) = match ev {
            RuntimeEvent::Input { session_id, input } => {
                // 契約 §34.5: Spawned を「観測した時点」で記録する。**遷移表を引く前に置くこと。**
                // running + Spawned は遷移なし(None)で continue するため、
                // 遷移後に置くとこの経路が黙って抜ける。
                // 冪等性は DAO 側の SQL(WHERE first_started_at IS NULL)が担保する。
                // SurfaceKind::Agent 限定は Spawned の発行元(start_session / resume_session)が
                // すでに構造的に保証しているので、ここで surface を再チェックしない(§34.5)。
                if input == StateInput::Spawned {
                    if let Err(err) = persist.mark_first_started(&session_id) {
                        eprintln!(
                            "[kamux] failed to record first_started_at for {}: {}",
                            session_id, err
                        );
                    }
                }

                let current = read_map(&states)
                    .get(&session_id)
                    .copied()
                    .unwrap_or(RuntimeState::Idle);

                let Some((next, reason)) = next_state(current, input) else {
                    continue;
                };

                // 判定から書き込みまでの間に終了が始まっていたら、ここで捨てる(最後のゲート)。
                // メモリ・DB・observer のいずれも更新しない(計画 §4.4 の逐語)。
                if shutting_down.load(Ordering::SeqCst) {
                    continue;
                }

                write_map(&states).insert(session_id.clone(), next);

                if let Err(err) = persist.set_last_runtime_state(&session_id, next) {
                    // DB 書き込み失敗で状態機械を止めない。真実はメモリ側にある。
                    eprintln!(
                        "[kamux] failed to persist runtime_state for {}: {}",
                        session_id, err
                    );
                }

                (session_id, next, reason)
            }

            // 契約 §40.2.1: 遷移表を引かない。現在状態が何であれ ❌ を確定させる
            // (`interrupted` からの再開失敗も ❌ になる。契約 §40.4)。
            // `mark_first_started` は呼ばない —— 起動していないのだから
            // (契約 §34.5 / §40.5。ここで立てると「最初の spawn **成功**の記録」という
            // `first_started_at` の意味が壊れ、M3-3 以降が誤読する)。
            RuntimeEvent::Error {
                session_id,
                message,
            } => {
                // Input 経路とまったく同じ最後のゲート。ここは先頭ゲートの直後だが、
                // `shutting_down` は他スレッドがいつでも立てるので、書き込みへ最も
                // 近い位置での再確認として残す(計画 §4.4 / 契約 §40.2.1「ゲートは
                // Input 経路とまったく同じにする」)。
                if shutting_down.load(Ordering::SeqCst) {
                    continue;
                }

                write_map(&states).insert(session_id.clone(), RuntimeState::Error);

                // `set_last_runtime_state(id, Error)` は使わない ——
                // メッセージの無い error 行ができる(契約 §17 / §38.8)。
                if let Err(err) = persist.set_runtime_error(&session_id, &message) {
                    // Input 経路と同じ: DB 書き込み失敗で状態機械を止めない。
                    eprintln!(
                        "[kamux] failed to persist runtime_error for {}: {}",
                        session_id, err
                    );
                }

                (session_id, RuntimeState::Error, StateReason::SpawnFailed)
            }
        };

        let payload = SessionStatePayload {
            session_id,
            runtime_state: next,
            reason,
        };
        for observer in observers.read().unwrap_or_else(|e| e.into_inner()).iter() {
            observer.on_state(&payload);
        }
    }
}

/// 契約 §8 のトピック表記。`{}` はランタイム置換（`pty/sink.rs` の `data_topic` と同じ形）。
pub fn state_topic(session_id: &str) -> String {
    format!("session://state/{session_id}")
}

/// 確定した遷移を WebView へ配る observer（契約 §8）。
///
/// **M2-3 の通知はこの型に行を足す形にしないこと。** 別の `StateObserver` 実装を
/// `RuntimeStateManager::register_observer` で足す —— それが observer を
/// `Vec<Arc<dyn StateObserver>>` にしてある理由である。
///
/// `R: Runtime` はテストのために足した内部実装の一般化であり、契約上の型ではない
/// （`TauriSink<R: Runtime = Wry>`（`pty/sink.rs`）と同じ前例）。デフォルト型パラメータが
/// `Wry` なので production の `TauriEmitObserver::new(app.handle().clone())` という
/// 呼び出し方は変わらない。`R` を `Wry` に固定すると `tauri::test::mock_builder()`
/// （`MockRuntime`）で実 emit を検証できない。
pub struct TauriEmitObserver<R: tauri::Runtime = tauri::Wry> {
    app: tauri::AppHandle<R>,
}

impl<R: tauri::Runtime> TauriEmitObserver<R> {
    pub fn new(app: tauri::AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: tauri::Runtime> StateObserver for TauriEmitObserver<R> {
    fn on_state(&self, payload: &SessionStatePayload) {
        use tauri::Emitter;
        // 契約 §8: 必ず `Emitter::emit`（グローバル）。`emit_to`（ウィンドウ指定）は使わない。
        // 送信失敗（WebView 破棄など）は無視する —— `TauriSink` と同じ方針で、
        // 状態機械を WebView の生死に巻き込まない。
        let _ = self
            .app
            .emit(&state_topic(&payload.session_id), payload.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RuntimeState::*;
    use StateInput as In;

    /// 契約 §8 のトピック文字列を逐語で守る。`session-state` や `session:state` は禁止（契約 §22）。
    #[test]
    fn state_topic_is_contract_verbatim() {
        assert_eq!(
            state_topic("3f2a9c1e-0000-4000-8000-000000000001"),
            "session://state/3f2a9c1e-0000-4000-8000-000000000001"
        );
    }

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

    /// `RuntimeEvent` は Task 11 で enum になった（契約 §40.2.1）。フィールドアクセスを
    /// ここへ集約してあったので、enum 化の書き換え面はこの 1 箇所で済む。
    /// `Input` 以外が来たら panic する（この節のテストは `Input` しか送らない）。
    fn expect_input(ev: RuntimeEvent) -> (String, StateInput) {
        match ev {
            RuntimeEvent::Input { session_id, input } => (session_id, input),
            other => panic!("expected Input, got {other:?}"),
        }
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
        /// `list_ids_by_last_runtime_state` が返した id の履歴(呼ばれた順)。
        /// 「全行を読み切ってから書く」の分離が崩れていないかを検証するための
        /// 観測チャネル(変異検証専用)。読みながら書く実装では、昇格した行が
        /// 同じ走査後半の `Interrupted` パスで再度読まれ、id が重複して現れる。
        reads: Mutex<Vec<String>>,
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
        fn reads(&self) -> Vec<String> {
            self.reads.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
        /// 契約 §38.7 の申し送り → §40.6 で確定。`writes()` / `first_started()` と同じ形。
        fn errors(&self) -> Vec<(String, String)> {
            self.errors
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
        fn replace_rows(&self, rows: &[(&str, RuntimeState)]) {
            *self.rows.lock().unwrap_or_else(|e| e.into_inner()) =
                rows.iter().map(|(k, v)| ((*k).to_string(), *v)).collect();
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
            // 実 DB(UPDATE sessions SET ...)を模す: 書き込みは以降の
            // list_ids_by_last_runtime_state から見える。normalize_on_startup が
            // 「全行を読み切ってから書く」を守っているかをこのフェイクだけで検証できるようにする。
            if let Some(row) = self
                .rows
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter_mut()
                .find(|(id, _)| id == session_id)
            {
                row.1 = state;
            }
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
            if self.fail.load(Ordering::SeqCst) {
                return Err(crate::error::AppError::Db("boom".into()));
            }
            let ids: Vec<String> = self
                .rows
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .filter(|(_, s)| *s == state)
                .map(|(id, _)| id.clone())
                .collect();
            self.reads
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend(ids.iter().cloned());
            Ok(ids)
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
        // 1 回目の書き込みは fail=true で失敗するが、observer 通知は
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
        // 見ていること。write_map(メモリ) は persist(DB) より前に確定するので、
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

    #[test]
    fn normalize_on_startup_promotes_running_and_waiting_input() {
        let persist = FakePersist::with_rows(&[
            ("live", Running),
            ("waiting", WaitingInput),
            ("fresh", Idle),
            ("dead", Exited),
            ("already", Interrupted),
        ]);
        let mgr = RuntimeStateManager::new(persist.clone());

        let mut changed = mgr.normalize_on_startup().expect("normalize");
        changed.sort_by(|a, b| a.0.cmp(&b.0));

        assert_eq!(
            changed,
            vec![
                ("live".to_string(), Interrupted),
                ("waiting".to_string(), Interrupted),
            ]
        );
        // 据え置き分は DB に書かない
        let mut writes = persist.writes();
        writes.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            writes,
            vec![
                ("live".to_string(), Interrupted),
                ("waiting".to_string(), Interrupted),
            ]
        );
    }

    /// 正規化されなかった行も含め、全セッションのメモリ上スナップショットを埋める。
    #[test]
    fn normalize_on_startup_seeds_in_memory_snapshot_for_all_rows() {
        let persist = FakePersist::with_rows(&[
            ("live", Running),
            ("fresh", Idle),
            ("dead", Exited),
            ("already", Interrupted),
        ]);
        let mgr = RuntimeStateManager::new(persist);

        mgr.normalize_on_startup().expect("normalize");

        assert_eq!(mgr.current("live"), Interrupted);
        assert_eq!(mgr.current("fresh"), Idle);
        assert_eq!(mgr.current("dead"), Exited);
        assert_eq!(mgr.current("already"), Interrupted);
    }

    /// `error` 行もメモリ上のスナップショットへ必ず載る（契約 §2 / §40.5）。
    /// `ALL_STATES` から `Error` を落とすと、この行は `list_ids_by_last_runtime_state`
    /// で一切読まれず `current("broken")` が `Idle` を返す(split-brain)。フロントは
    /// `list_sessions` から ❌ を seed するため、Rust 側だけが食い違う。
    #[test]
    fn normalize_on_startup_seeds_error_rows_into_memory_snapshot() {
        let persist = FakePersist::with_rows(&[("broken", Error)]);
        let mgr = RuntimeStateManager::new(persist.clone());

        let changed = mgr.normalize_on_startup().expect("normalize");

        // error は §40.5 のとおり据え置き(promote されない)
        assert!(changed.is_empty());
        assert!(persist.writes().is_empty());
        assert_eq!(mgr.current("broken"), Error);
    }

    /// 全行を読み切ってから書き込む。正規化で interrupted にした行を
    /// 同じ走査の中で読み直して二重処理しない。
    #[test]
    fn normalize_on_startup_does_not_reprocess_rows_it_just_wrote() {
        let persist = FakePersist::with_rows(&[("live", Running)]);
        let mgr = RuntimeStateManager::new(persist.clone());

        let changed = mgr.normalize_on_startup().expect("normalize");

        assert_eq!(changed, vec![("live".to_string(), Interrupted)]);
        assert_eq!(persist.writes().len(), 1, "同じ行に 2 回書いてはいけない");
        // 「全行を読み切ってから書く」の分離が保たれているかを読み出し履歴で固定する。
        // 読みながら書く実装だと、interrupted に更新した "live" が同じ走査後半の
        // Interrupted パスで再度読まれ、id が重複して現れる。
        assert_eq!(
            persist.reads(),
            vec!["live".to_string()],
            "昇格した行を同じ走査内で読み直してはいけない"
        );
    }

    /// 2 回続けて起動しても結果が変わらない（クラッシュ後の再々起動）。
    #[test]
    fn normalize_on_startup_is_idempotent_across_restarts() {
        let persist = FakePersist::with_rows(&[("live", Running)]);
        let mgr = RuntimeStateManager::new(persist.clone());
        mgr.normalize_on_startup().expect("1 回目");

        // 1 回目の書き込みを DB 側に反映させたうえで、新しいマネージャで再実行する
        persist.replace_rows(&[("live", Interrupted)]);
        let mgr2 = RuntimeStateManager::new(persist.clone());
        let changed = mgr2.normalize_on_startup().expect("2 回目");

        assert!(changed.is_empty());
        assert_eq!(mgr2.current("live"), Interrupted);
    }

    /// 正規化直後に開始すると interrupted -> running へ抜けられる。
    #[test]
    fn normalized_session_can_be_restarted() {
        let persist = FakePersist::with_rows(&[("live", Running)]);
        let mgr = RuntimeStateManager::new(persist);
        mgr.normalize_on_startup().expect("normalize");

        mgr.sender().send("live", In::Spawned);
        assert!(wait_until(|| mgr.current("live") == Running));
    }

    /// 契約 §8: `session://state/{session_id}` を **`Emitter::emit`（グローバル）** で発火する。
    /// `emit_to`（ウィンドウ指定）へ「最適化」するとフロントの購読と M2-2 以降が壊れる。
    ///
    /// トピックに `session_id` を埋めて宛先を表現する設計なので、別セッションの
    /// トピックへ漏れないことも同時に固定する。
    #[test]
    fn tauri_emit_observer_emits_the_payload_on_the_contract_topic() {
        use std::sync::mpsc::channel;
        use tauri::test::{mock_builder, mock_context, noop_assets};
        use tauri::Listener;

        let app = mock_builder()
            .build(mock_context(noop_assets()))
            .expect("build mock app");

        let (tx, rx) = channel::<String>();
        app.listen(state_topic("s1"), move |event| {
            let _ = tx.send(event.payload().to_string());
        });
        let (other_tx, other_rx) = channel::<String>();
        app.listen(state_topic("s2"), move |event| {
            let _ = other_tx.send(event.payload().to_string());
        });

        let observer = TauriEmitObserver::new(app.handle().clone());
        observer.on_state(&SessionStatePayload {
            session_id: "s1".to_string(),
            runtime_state: WaitingInput,
            reason: StateReason::HookPermission,
        });

        let raw = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("session://state/s1 が発火するはず");
        let payload: serde_json::Value = serde_json::from_str(&raw).expect("payload json");
        assert_eq!(payload["session_id"], serde_json::json!("s1"));
        assert_eq!(payload["runtime_state"], serde_json::json!("waiting_input"));
        assert_eq!(payload["reason"], serde_json::json!("hook_permission"));
        assert!(
            other_rx.try_recv().is_err(),
            "別セッションのトピックへ流してはいけない"
        );
    }

    /// 計画 §4.9: 起動時正規化はイベントを emit しない。
    ///
    /// この関数は WebView がまだ `listen` を張っていない時点で走るので、出しても誰も
    /// 受け取れない。フロントは `list_sessions` の戻り値（正規化済みの
    /// `last_runtime_state`）からシードする。
    ///
    /// `normalize_on_startup` は `&self` から `self.observers` に到達できるため、
    /// 将来ここに emit を足してもコンパイルは通ってしまう。**この禁止を守っている
    /// テストはこれ 1 本だけである**（Task 5 レビュー持ち越し / Task 6 必達 3）。
    #[test]
    fn normalize_on_startup_notifies_no_observer() {
        let persist = FakePersist::with_rows(&[
            ("live", Running),
            ("waiting", WaitingInput),
            ("fresh", Idle),
            ("broken", Error),
        ]);
        let mgr = RuntimeStateManager::new(persist.clone());
        let obs = Arc::new(RecordingObserver::default());
        mgr.register_observer(obs.clone());

        let changed = mgr.normalize_on_startup().expect("normalize");

        // 陽性コントロール: 正規化自体は起きている(= emit を足せる遷移が実在する)
        assert_eq!(changed.len(), 2, "live / waiting が interrupted になるはず");
        assert!(
            obs.seen().is_empty(),
            "起動時正規化は observer を呼んではいけない(計画 §4.9): {:?}",
            obs.seen()
        );
    }

    /// DB 読み出しが失敗したら、状態を捏造せずエラーを返す。
    /// `Exited` は `normalize_startup_state` で据え置き(None)になり書き込みを一切
    /// 起こさないので、`fail` が捕まえるのは読み取り経路の失敗だけになる
    /// （`Running` など昇格する行だと `set_last_runtime_state` 側の失敗伝播と
    /// 見分けが付かない。PR 13 レビュー I-3）。
    #[test]
    fn normalize_on_startup_propagates_persist_errors() {
        let persist = FakePersist::with_rows(&[("dead", Exited)]);
        persist.fail.store(true, Ordering::SeqCst);
        let mgr = RuntimeStateManager::new(persist);

        assert!(mgr.normalize_on_startup().is_err());
    }

    // --- Task 7: `sink.rs` / `write_pty` / `stop_session` から届く入力の形 ---
    //
    // 待ち方は契約 §69.1 / §69.2 に従う。副作用の順は memory -> DB(persist) ->
    // observer なので、正の主張では **assert する対象そのもの**を待つ。
    // 負の主張（「起きてほしくない作用が起きていない」）には待つ条件が作れないため
    // 素の `sleep` を使う —— `wait_until` に書き換えると条件成立の瞬間に抜けてしまい、
    // 誤った作用が後から来ても検出できなくなる（§69.2）。

    /// agent サーフェスの PTY 終了でセッションが ⛔ になる。
    /// `sink.rs` の `on_exit` は surface_id をそのまま渡してくる。
    #[test]
    fn agent_pty_exit_marks_session_exited() {
        let persist = FakePersist::with_rows(&[]);
        let mgr = RuntimeStateManager::new(persist.clone());
        let sender = mgr.sender();

        sender.send("s1", In::Spawned);
        assert!(wait_until(
            || persist.writes() == vec![("s1".to_string(), Running)]
        ));

        sender.note_pty_exit("s1:agent");

        assert!(wait_until(|| persist.writes()
            == vec![
                ("s1".to_string(), Running),
                ("s1".to_string(), Exited)
            ]));
        // メモリは DB より先に確定するので、writes() が観測できた時点で安定している
        assert_eq!(mgr.current("s1"), Exited);
    }

    /// `stop_session` は kill したあと `UserStopped` を送るが、
    /// その直後に PTY が実際に死んで `on_exit` から `PtyExited` も届く。
    /// 2 通目は `exited` + `PtyExited` = 遷移なしなので、DB もイベントも動かない。
    #[test]
    fn stop_session_followed_by_pty_exit_writes_only_once() {
        let persist = FakePersist::with_rows(&[]);
        let mgr = RuntimeStateManager::new(persist.clone());
        let obs = Arc::new(RecordingObserver::default());
        mgr.register_observer(obs.clone());
        let sender = mgr.sender();

        sender.send("s1", In::Spawned);
        sender.send("s1", In::UserStopped);
        // observer が最後の副作用。ここまで届いたことを待ってから負の主張に移る
        assert!(wait_until(|| obs.seen().len() == 2));

        sender.note_pty_exit("s1:agent");

        // 負の主張なので待つ条件が作れない（契約 §69.2）
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(mgr.current("s1"), Exited);
        assert_eq!(
            persist.writes(),
            vec![("s1".to_string(), Running), ("s1".to_string(), Exited)],
            "PtyExited が後追いで来ても 2 度目の書き込みをしてはいけない"
        );
        assert_eq!(obs.seen().len(), 2);
        // 先に来た UserStopped の reason が残る
        assert_eq!(obs.seen()[1].2, StateReason::UserStopped);
    }

    /// 既に ⛔ のセッションへもう一度停止ボタンを撃つ経路。
    /// `PtyManager::kill` は冪等（契約 §15）なので、登録されていない surface_id でも
    /// `Ok(())` が返り `stop_session` は `UserStopped` を送る。状態機械は
    /// `exited` + `UserStopped` を遷移なしで捨てるので、DB もイベントも動かない。
    #[test]
    fn a_second_user_stop_after_exit_writes_nothing_more() {
        let persist = FakePersist::with_rows(&[]);
        let mgr = RuntimeStateManager::new(persist.clone());
        let obs = Arc::new(RecordingObserver::default());
        mgr.register_observer(obs.clone());
        let sender = mgr.sender();

        sender.send("s1", In::Spawned);
        sender.send("s1", In::UserStopped);
        assert!(wait_until(|| obs.seen().len() == 2));

        sender.send("s1", In::UserStopped);

        // 負の主張なので待つ条件が作れない（契約 §69.2）
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(mgr.current("s1"), Exited);
        assert_eq!(
            persist.writes(),
            vec![("s1".to_string(), Running), ("s1".to_string(), Exited)],
            "2 度目の停止で DB を書き直してはいけない"
        );
        assert_eq!(obs.seen().len(), 2, "2 度目の停止でバッジを再更新しない");
    }

    /// nvim を閉じてもセッションは ⛔ にならない（設計書 §6.4 / 契約 §2）。
    /// `sink.rs` にはフィルタを書かないので、この判定は note_surface の責務。
    #[test]
    fn editor_surface_exit_does_not_change_session_state() {
        let persist = FakePersist::with_rows(&[]);
        let mgr = RuntimeStateManager::new(persist.clone());
        let sender = mgr.sender();

        sender.send("s1", In::Spawned);
        assert!(wait_until(
            || persist.writes() == vec![("s1".to_string(), Running)]
        ));

        sender.note_pty_exit("s1:editor");

        // 負の主張なので待つ条件が作れない（契約 §69.2）
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(mgr.current("s1"), Running);
        assert_eq!(persist.writes(), vec![("s1".to_string(), Running)]);
    }

    /// 契約 §41.3: 再開失敗の終了は `exited` へ落ちるが reason が `ResumeFailed` になる。
    /// M2-4 のフロントはこの reason だけを見て再試行導線を出すので、
    /// `PtyExited` に丸めるとリカバリ導線が丸ごと死ぬ。
    #[test]
    fn resume_failed_exit_reports_its_own_reason() {
        let persist = FakePersist::with_rows(&[]);
        let mgr = RuntimeStateManager::new(persist);
        let obs = Arc::new(RecordingObserver::default());
        mgr.register_observer(obs.clone());
        let sender = mgr.sender();

        sender.send("s1", In::Spawned);
        assert!(wait_until(|| obs.seen().len() == 1));

        sender.note_resume_failed_exit("s1:agent");

        assert!(wait_until(|| obs.seen().len() == 2));
        assert_eq!(obs.seen()[1].1, Exited);
        assert_eq!(obs.seen()[1].2, StateReason::ResumeFailed);
    }

    /// `note_pty_exit` と同じ `:agent` フィルタを通ることの固定。
    #[test]
    fn resume_failed_exit_ignores_the_editor_surface() {
        let persist = FakePersist::with_rows(&[]);
        let mgr = RuntimeStateManager::new(persist.clone());
        let sender = mgr.sender();

        sender.send("s1", In::Spawned);
        assert!(wait_until(
            || persist.writes() == vec![("s1".to_string(), Running)]
        ));

        sender.note_resume_failed_exit("s1:editor");

        // 負の主張なので待つ条件が作れない（契約 §69.2）
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(mgr.current("s1"), Running);
        assert_eq!(persist.writes(), vec![("s1".to_string(), Running)]);
    }

    /// write_pty 由来の UserInput が 🟡 を解除する。出力では解除されない。
    #[test]
    fn user_input_clears_waiting_input_but_output_does_not() {
        let persist = FakePersist::with_rows(&[]);
        let mgr = RuntimeStateManager::new(persist);
        let sender = mgr.sender();

        sender.send("s1", In::Spawned);
        sender.send("s1", In::HookNotification);
        assert!(wait_until(|| mgr.current("s1") == WaitingInput));

        // TUI の再描画では 🟡 は消えない
        sender.note_surface("s1:agent", In::OutputActivity);

        // 負の主張なので待つ条件が作れない（契約 §69.2）
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(mgr.current("s1"), WaitingInput);

        // ユーザーが打鍵したら 🟢 に戻る
        sender.note_user_input("s1:agent");
        assert!(wait_until(|| mgr.current("s1") == Running));
    }

    // --- Task 11: `mark_error` —— `error` ❌ への唯一の到達経路（契約 §40） ---

    /// ❌ は遷移表を経由せずに確定し、生 stderr が永続化され、SpawnFailed で emit される。
    #[test]
    fn mark_error_sets_error_state_with_raw_message() {
        let persist = FakePersist::with_rows(&[]);
        let mgr = RuntimeStateManager::new(persist.clone());
        let obs = Arc::new(RecordingObserver::default());
        mgr.register_observer(obs.clone());
        let sender = mgr.sender();

        sender.mark_error("s1", "claude: command not found\n");

        assert!(wait_until(|| mgr.current("s1") == RuntimeState::Error));
        assert_eq!(
            persist.errors(),
            vec![("s1".to_string(), "claude: command not found\n".to_string())],
            "stderr は加工せずそのまま渡す（契約 §2 / §6）"
        );
        // set_last_runtime_state 経由で書いてはいけない（メッセージの無い error 行ができる）
        assert!(persist.writes().is_empty());
        assert!(wait_until(|| obs.seen().len() == 1));
        assert_eq!(obs.seen()[0].1, RuntimeState::Error);
        assert_eq!(obs.seen()[0].2, StateReason::SpawnFailed);
    }

    /// 起動していないのだから first_started_at は立てない（契約 §34.5 / §40.5）。
    #[test]
    fn mark_error_does_not_record_first_started() {
        let persist = FakePersist::with_rows(&[]);
        let mgr = RuntimeStateManager::new(persist.clone());

        mgr.sender().mark_error("s1", "boom");

        assert!(wait_until(|| mgr.current("s1") == RuntimeState::Error));
        assert!(persist.first_started().is_empty());
        assert_eq!(persist.first_started_calls(), 0);
    }

    /// ❌ から抜けられるのは Spawned だけ。他の入力では ❌ のまま。
    #[test]
    fn error_state_is_left_only_by_spawned() {
        let persist = FakePersist::with_rows(&[]);
        let mgr = RuntimeStateManager::new(persist.clone());
        let sender = mgr.sender();

        sender.mark_error("s1", "boom");
        assert!(wait_until(|| mgr.current("s1") == RuntimeState::Error));

        // 遅れて届く PTY 終了や hook で ❌ を上書きしない
        sender.send("s1", In::PtyExited);
        sender.send("s1", In::HookNotification);
        // 負の主張なので待つ条件が作れない（契約 §69.2）
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(mgr.current("s1"), RuntimeState::Error);

        sender.send("s1", In::Spawned);
        assert!(wait_until(|| mgr.current("s1") == Running));
    }

    /// 中断中のセッションの再開失敗も ❌ になる（契約 §40.4。M2-1 §4.12 の決着）。
    #[test]
    fn mark_error_overwrites_interrupted() {
        let persist = FakePersist::with_rows(&[("s1", Running)]);
        let mgr = RuntimeStateManager::new(persist.clone());
        mgr.normalize_on_startup().expect("normalize");
        assert_eq!(mgr.current("s1"), RuntimeState::Interrupted);

        mgr.sender().mark_error("s1", "cwd not found");

        assert!(wait_until(|| mgr.current("s1") == RuntimeState::Error));
        assert_eq!(persist.errors().len(), 1);
    }

    /// 終了処理が始まったら ❌ も書かない（Input 経路と同じゲート）。
    #[test]
    fn mark_error_is_dropped_after_begin_shutdown() {
        let persist = FakePersist::with_rows(&[]);
        let mgr = RuntimeStateManager::new(persist.clone());

        mgr.begin_shutdown();
        mgr.sender().mark_error("s1", "boom");

        // 負の主張なので待つ条件が作れない（契約 §69.2）
        std::thread::sleep(Duration::from_millis(50));
        assert!(persist.errors().is_empty());
        assert_eq!(mgr.current("s1"), RuntimeState::Idle);
    }

    /// 大量打鍵しても、running 中の UserInput は送信前に捨てられる。
    #[test]
    fn repeated_keystrokes_while_running_produce_no_events() {
        let persist = FakePersist::with_rows(&[]);
        let mgr = RuntimeStateManager::new(persist.clone());
        let sender = mgr.sender();

        sender.send("s1", In::Spawned);
        assert!(wait_until(
            || persist.writes() == vec![("s1".to_string(), Running)]
        ));

        for _ in 0..1000 {
            sender.note_user_input("s1:agent");
        }

        // 負の主張なので待つ条件が作れない（契約 §69.2）
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(persist.writes(), vec![("s1".to_string(), Running)]);
    }
}
