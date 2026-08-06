export interface EditorTerminalLike {
  options: { macOptionIsMeta?: boolean };
}

/**
 * エディタ用サーフェス固有の設定を適用する。
 * 契約 §16 の ensureTerminal にオプション引数が無いため、生成後に options へ代入する。
 * options への代入だけなので何度呼んでも同じ結果になる（設計判断 D10）。
 *
 * ここで設定しないもの:
 *   - scrollback  : M1-3 の ensureTerminal が surface_id の `:editor` 接尾辞を見て
 *                   1,000 行に切り替える（契約 §19）。二重に持つと真実源が分かれる
 *   - term.onData : ensureTerminal が既に接続済み。二重接続すると打鍵が 2 回 PTY に
 *                   届き、nvim で `dd` が `dddd` になる（契約 §16 / 設計判断 D12）
 *   - キーハンドラ : ensureTerminal が attachCustomKeyEventHandler を全サーフェスに
 *                   設定済みで、そのハンドラは _keyDownSeen を戻す副作用を持つ
 *                   （契約 §65.3 / PR H1 / §77）。attachCustomKeyEventHandler は
 *                   ハンドラを 1 つだけ保持するセッターなので、ここで再設定すると
 *                   副作用が上書きで落ち、nvim のインサートモードで
 *                   `!` / `@` / Shift+英字の 1 打目が消える（契約 §65.6 I3）
 */
export function applyEditorTerminalOptions(term: EditorTerminalLike): void {
  term.options.macOptionIsMeta = true;
}
