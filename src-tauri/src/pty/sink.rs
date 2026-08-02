// Task 8 で TauriSink 本実装(PtyDataPayload / PtyExitPayload の emit)を追加する。
// Task 6 の時点では PtyManager::spawn が AppHandle を受け取れるようにするための
// 最小のプレースホルダのみを置く。
use std::sync::Arc;

use tauri::AppHandle;

use crate::pty::surface::PtySink;

pub struct TauriSink {
    #[allow(dead_code)]
    app: AppHandle,
}

impl TauriSink {
    pub fn new(app: AppHandle) -> Arc<Self> {
        Arc::new(Self { app })
    }
}

impl PtySink for TauriSink {
    fn on_data(&self, _surface_id: &str, _base64: String, _seq: u64) {}
    fn on_exit(&self, _surface_id: &str, _exit_code: Option<i32>) {}
}
