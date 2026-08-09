use std::sync::Arc;

use crate::hooks_srv::HooksRuntime;
use crate::pty::PtyManager;
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
    /// hooks が有効なとき Some。relay 解決に失敗した場合や、Task 13 のブートストラップが
    /// まだ値を渡していない起動経路では None（契約 §75.5 / §84.6.2）。
    /// `Mutex` は不要 —— 起動時に確定し、以後書き換わらない（§84.6.2 の 3 箇所目）。
    /// `set_hooks` / `hooks()` のような専用メソッドは作らない。`state.hooks.as_ref()` が
    /// 唯一の到達経路（契約 §75 の適用）。
    pub hooks: Option<HooksRuntime>,
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::{AppState, Arc, PtyManager, RuntimeStateManager, Store};

    /// 実 `Store` を `StatePersist` に差した `AppState`。
    /// production の `install_app_state` と同じ結線（persist = その `Store` 自身）を
    /// テスト側で 1 行にする。`normalize_on_startup` は呼ばない —— 起動時正規化を
    /// 検証するテストは `install_app_state` 側を通ること。`hooks` は既定で `None`
    /// （hooks を使うテストは `state.hooks = Some(..)` を直接代入する）。
    pub(crate) fn app_state(store: Store) -> AppState {
        let store = Arc::new(store);
        AppState {
            store: Arc::clone(&store),
            pty: PtyManager::new(),
            runtime: RuntimeStateManager::new(store),
            hooks: None,
        }
    }
}
