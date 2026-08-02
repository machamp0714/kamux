// src-tauri/src/pty/commands.rs
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use tauri::State;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// xterm の onData（UTF-8 文字列）をそのまま PTY に書く
#[tauri::command]
pub async fn write_pty(
    state: State<'_, AppState>,
    surface_id: String,
    data: String,
) -> AppResult<()> {
    state.pty.write(&surface_id, data.as_bytes())
}

/// xterm の onBinary（非 UTF-8 バイト列）を base64 経由で PTY に書く（契約への追加提案 4）
#[tauri::command]
pub async fn write_pty_bytes(
    state: State<'_, AppState>,
    surface_id: String,
    base64: String,
) -> AppResult<()> {
    let bytes = BASE64
        .decode(base64.as_bytes())
        .map_err(|e| AppError::Io(format!("invalid base64 payload: {e}")))?;
    state.pty.write(&surface_id, &bytes)
}

#[tauri::command]
pub async fn resize_pty(
    state: State<'_, AppState>,
    surface_id: String,
    cols: u16,
    rows: u16,
) -> AppResult<()> {
    state.pty.resize(&surface_id, cols, rows)
}

/// フロントが seq まで消化したことを通知する（バックプレッシャー解除、契約 §9）
#[tauri::command]
pub async fn ack_pty(state: State<'_, AppState>, surface_id: String, seq: u64) -> AppResult<()> {
    state.pty.ack(&surface_id, seq)
}
