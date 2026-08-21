use std::sync::{Arc, Mutex};

use crate::hooks_srv::{HooksRuntime, HooksServer};
use crate::pty::PtyManager;
use crate::session::heuristics::registry::HeuristicRegistry;
use crate::session::runtime_state::RuntimeStateManager;
use crate::store::Store;

/// Tauri の manage 対象。
/// 契約 §17: Store は Arc で保持する。M1-3 / M2-1 がバックグラウンドスレッドへ
/// clone を渡す必要があるため（中身の Mutex<Connection> は Send + Sync）。
pub struct AppState {
    pub store: Arc<Store>,
    pub pty: PtyManager,
    /// runtime_state の直列化層（M2-1）。`sink.rs` と各コマンドは
    /// `state.runtime.sender()` で状態機械の入口を取る。
    ///
    /// 契約 §41.1 の表は同じ入口を `SessionManager::runtime_sender()` と書いているが、
    /// M1-4 は `SessionManager` 型を作らず `session/mod.rs` の自由関数コマンド +
    /// `AppState` という形で実装した。呼び出し元の無い委譲メソッドのためだけに
    /// 空の型を作らず、`AppState.runtime` を唯一の到達経路とする（Task 6 report 参照）。
    pub runtime: Arc<RuntimeStateManager>,
    /// 汎用 CLI ヒューリスティックの統括（M3-3）。**`RuntimeStateManager` の中には
    /// 入れない** —— あちらは状態機械の中核で、ヒューリスティックは*そこへ入力を
    /// 流し込む側*である。中に入れると層が反転し、かつ循環が生まれる。
    ///
    /// `heuristics()` のような専用メソッドは作らない。`state.heuristics` が唯一の
    /// 到達経路である（`hooks` と同じ、契約 §75 の適用）。
    pub heuristics: Arc<HeuristicRegistry>,
    /// hooks が有効なとき Some。relay 解決に失敗した場合や、Task 13 のブートストラップが
    /// まだ値を渡していない起動経路では None（契約 §75.5 / §84.6.2）。
    /// `Mutex` は不要 —— 起動時に確定し、以後書き換わらない（§84.6.2 の 3 箇所目）。
    /// `set_hooks` / `hooks()` のような専用メソッドは作らない。`state.hooks.as_ref()` が
    /// 唯一の到達経路（契約 §75 の適用）。
    pub hooks: Option<HooksRuntime>,
    /// 再開試行の追跡（M2-4）。**`RuntimeStateManager` の中には入れない** ——
    /// あちらは状態機械の中核で、こちらは「その終了が resume 失敗か」を
    /// 分類して入力を選ぶ側である（`heuristics` と同じ層の関係）。
    ///
    /// `Arc` なのは `HookHandler` が同じ実体を持つ必要があるためである。
    /// handler は `AppHandle` を持たないので `AppState` から読めず、
    /// `HookHandler::new` の引数として渡すのが唯一の経路になる。
    /// setter は作らない（`state.resume_tracker` が唯一の到達経路。契約 §75）。
    pub resume_tracker: Arc<crate::session::resume_tracker::ResumeTracker>,
    /// 通知の判定・送信と Dock バッジの数え上げ（M2-3）。**`RuntimeStateManager` の中には
    /// 入れない** —— あちらは状態機械の中核で、こちらはその出力を購読する側である
    /// （`heuristics` / `resume_tracker` と同じ層の関係）。
    ///
    /// `Arc` なのは、同じ実体を `NotifyObserver`（`RuntimeStateManager` の observer）と
    /// 共有する必要があるためである。observer は `AppState` を読めないので、
    /// `install_app_state_with` が両方へ同じ `Arc` を配るのが唯一の経路になる。
    /// setter は作らない（`state.notifier` が唯一の到達経路。契約 §75）。
    pub notifier: Arc<crate::notify::Notifier>,
    /// Drop でソケットを unlink するため保持する。`shutdown()` が `&mut self` を要る
    /// ため `hooks` とは違い `Mutex` が必要（RunEvent::Exit のハンドラから止める）。
    pub hooks_server: Mutex<Option<HooksServer>>,
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::{AppState, Arc, HeuristicRegistry, Mutex, PtyManager, RuntimeStateManager, Store};

    /// 実 `Store` を `StatePersist` に差した `AppState`。
    /// production の `install_app_state` と同じ結線（persist = その `Store` 自身）を
    /// テスト側で 1 行にする。`normalize_on_startup` は呼ばない —— 起動時正規化を
    /// 検証するテストは `install_app_state` 側を通ること。`hooks` / `hooks_server` は
    /// 既定で無効（hooks を使うテストは `state.hooks = Some(..)` を直接代入する）。
    ///
    /// `heuristics` は production と同じ `ManagerSink` + `SystemClock` で組む。
    /// ランタイムハンドルは `tauri::async_runtime::handle()`（`Handle::current()` は
    /// 同期テストから呼ぶと panic する）。**仮想時間で沈黙タイムアウトを見るテストは
    /// ここを通さず、`HeuristicRegistry::new` を自分で組むこと** —— この registry の
    /// 消費ループは tauri のグローバルランタイム上に載るので `start_paused` が効かない。
    pub(crate) fn app_state(store: Store) -> AppState {
        let store = Arc::new(store);
        let runtime = RuntimeStateManager::new(Arc::clone(&store) as Arc<_>);
        let heuristics = HeuristicRegistry::new(
            Arc::new(crate::session::heuristics::clock::SystemClock),
            Arc::new(crate::session::heuristics::sink_impl::ManagerSink::new(
                runtime.sender(),
            )),
            tauri::async_runtime::handle().inner().clone(),
        );
        AppState {
            store,
            pty: PtyManager::new(),
            runtime,
            heuristics,
            resume_tracker: Arc::new(crate::session::resume_tracker::ResumeTracker::new()),
            // `M2-3-macos-notification.md:515`: テストでは OS 通知を実送信しない
            // [`RecordingSink`] に差し替える。
            // ラベルは常に None を返し、`Notifier` 側の `SessionLabel::fallback` に落とす
            // （このヘルパを使うテストは通知文言を主題にしていない）。
            notifier: Arc::new(crate::notify::Notifier::new(
                Arc::new(crate::notify::RecordingSink::default()),
                Arc::new(|_id: &str| None),
            )),
            hooks: None,
            hooks_server: Mutex::new(None),
        }
    }
}
