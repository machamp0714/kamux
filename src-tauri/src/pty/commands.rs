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
    state.pty.write(&surface_id, data.as_bytes())?;
    // 🟡 を解除できる唯一の経路。出力活動では解除しない（TUI の再描画で消えてしまうため）。
    // `note_user_input` が `:editor` を弾き、running 中の打鍵（遷移が起きない入力）も
    // 送信前に捨てる。**ここはキー入力ごとに呼ばれる高頻度経路である。**
    // 現在状態を read-lock で覗くだけの高速パスが契約 §0（アイドル CPU ほぼ 0%）の
    // 根拠なので、これ以上の処理を足さないこと。
    state.runtime.sender().note_user_input(&surface_id);
    Ok(())
}

/// xterm の onBinary（非 UTF-8 バイト列）を base64 経由で PTY に書く（契約への追加提案 4）。
///
/// **`note_user_input` は呼ばない。** xterm.js の `onBinary` は typings が明記するとおり
/// 「UTF-8 に収まらない一部のマウス報告」専用であって打鍵の経路ではない
/// （`@xterm/xterm/typings/xterm.d.ts`: "Currently this is only used for a certain type
/// of mouse reports that happen to be not UTF-8 compatible."）。これを `UserInput` として
/// 扱うと、端末上でマウスを動かす・スクロールするだけで 🟡 が解除されてしまう ——
/// 遷移表が `OutputActivity` で `waiting_input` を解除しない理由（契約 §2）と同じ事故になる。
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
