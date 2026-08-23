// src-tauri/src/pty/commands.rs
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use tauri::State;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// 端末が自分で生成する報告の閉じた集合（契約 §139.4）。**この配列 1 箇所だけに置く。**
/// 増やすときは、一次資料（xterm の `triggerDataEvent` 呼び出し位置）と、実際にその CLI が
/// 出したことの実測を、1 バイト列につき 1 組ずつ揃えてから 1 行足す（契約 §139.4.1）。
const TERMINAL_GENERATED_REPORTS: [&str; 2] = ["\x1b[I", "\x1b[O"];

/// `data` が端末エミュレータ自身の生成した報告（フォーカス報告）と payload 全体で
/// 完全一致するかどうかを判定する（契約 §139.2 / §139.4）。人間が発した入力ではないので、
/// `write_pty` はこれを `UserInput` として通知しない。
///
/// **前方一致・部分一致・正規表現の走査は使わない。** 人間が `Esc` `[` `I` と順に打つと、
/// xterm は 1 打鍵ごとに `triggerDataEvent` を呼ぶので `onData` へは 3 チャンクに分かれて
/// 届く（`@xterm/xterm/src/browser/Terminal.ts:1072` / `:1155`。いずれも `_keyDown` /
/// `_keyPress` の中で打鍵 1 回につき 1 回呼ばれる）。走査型はこの 3 打鍵目
/// （`"I"` だけの chunk）を `"\x1b[I"` の部分一致として拾ってしまい、🟡 が消えなくなる。
/// payload 全体の完全一致だけがこの事故を避けられる。
fn is_terminal_generated_report(data: &str) -> bool {
    TERMINAL_GENERATED_REPORTS.contains(&data)
}

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
    // 2 要素の完全一致（`is_terminal_generated_report`）は `note_user_input` の中の
    // read-lock より前に立ち、read-lock より安いので、契約 §0 のアイドル CPU は
    // 影響を受けない。ただし**判別式以外を足さないこと**。
    if !is_terminal_generated_report(&data) {
        state.runtime.sender().note_user_input(&surface_id);
    }
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

#[cfg(test)]
mod tests {
    use super::is_terminal_generated_report;

    #[test]
    fn is_terminal_generated_report_true_for_focus_in() {
        assert!(is_terminal_generated_report("\x1b[I"));
    }

    #[test]
    fn is_terminal_generated_report_true_for_focus_out() {
        assert!(is_terminal_generated_report("\x1b[O"));
    }

    #[test]
    fn is_terminal_generated_report_false_for_each_chunk_of_a_human_typing_esc_bracket_i() {
        // 人間が Esc [ I と順に打つと、xterm は 1 打鍵ごとに triggerDataEvent を呼ぶので
        // onData へは 3 チャンクに分かれて届く（契約 §139.4）。走査型ならここが緑になって
        // しまうが、この 3 本は「集合の要素そのものを増やす誤り」を殺すためにある。
        assert!(!is_terminal_generated_report("\x1b"));
        assert!(!is_terminal_generated_report("["));
        assert!(!is_terminal_generated_report("I"));
    }

    #[test]
    fn is_terminal_generated_report_false_for_forward_match_beyond_the_report() {
        // 前方一致（`starts_with`）や `contains` なら通ってしまう入力。完全一致だけが弾く。
        assert!(!is_terminal_generated_report("\x1b[Iy"));
    }

    #[test]
    fn is_terminal_generated_report_false_for_backward_match_before_the_report() {
        // 後方一致（`ends_with`）や `contains` なら通ってしまう入力。完全一致だけが弾く。
        assert!(!is_terminal_generated_report("y\x1b[I"));
    }

    #[test]
    fn is_terminal_generated_report_false_for_a_plain_keystroke() {
        assert!(!is_terminal_generated_report("y"));
    }
}
