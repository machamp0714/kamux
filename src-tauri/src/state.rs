use std::sync::Arc;

use crate::pty::PtyManager;
use crate::store::Store;

/// Tauri の manage 対象。
/// 契約 §17: Store は Arc で保持する。M1-3 / M2-1 がバックグラウンドスレッドへ
/// clone を渡す必要があるため（中身の Mutex<Connection> は Send + Sync）。
/// SessionManager（RuntimeStateManager）は M2-1 が追加する。
pub struct AppState {
    pub store: Arc<Store>,
    pub pty: PtyManager,
}
