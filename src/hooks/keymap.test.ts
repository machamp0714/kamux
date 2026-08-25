import { describe, expect, it } from 'vitest';
import { resolveKeymap, type KeymapEvent } from './keymap';

const closed = { modalOpen: false, view: 'kanban' as const };
const open = { modalOpen: true, view: 'kanban' as const };
const terminalView = { modalOpen: false, view: 'terminal' as const };
const editorView = { modalOpen: false, view: 'editor' as const };
const terminalModal = { modalOpen: true, view: 'terminal' as const };

/** ctrlKey / altKey を明示しない呼び出しを短く書くためのヘルパー。既定値は両方 false。 */
const ev = (over: Partial<KeymapEvent> & Pick<KeymapEvent, 'key' | 'metaKey'>): KeymapEvent => ({
  ctrlKey: false,
  altKey: false,
  ...over,
});

describe('resolveKeymap', () => {
  it('Cmd+1 でカンバン画面へ切り替える（契約 §11）', () => {
    expect(resolveKeymap(ev({ key: '1', metaKey: true }), closed)).toEqual({
      type: 'set_view',
      view: 'kanban',
    });
  });

  it('Cmd+2 でターミナル画面へ切り替える（契約 §11）', () => {
    expect(resolveKeymap(ev({ key: '2', metaKey: true }), closed)).toEqual({
      type: 'set_view',
      view: 'terminal',
    });
  });

  it('ターミナル画面では Cmd+J が cycleSession(1)', () => {
    expect(resolveKeymap(ev({ key: 'j', metaKey: true }), terminalView)).toEqual({
      type: 'cycle_session',
      dir: 1,
    });
  });

  it('ターミナル画面では Cmd+K が cycleSession(-1)', () => {
    expect(resolveKeymap(ev({ key: 'k', metaKey: true }), terminalView)).toEqual({
      type: 'cycle_session',
      dir: -1,
    });
  });

  it('Shift 併用で大文字になっても Cmd+J/K は効く', () => {
    expect(resolveKeymap(ev({ key: 'J', metaKey: true }), terminalView)).toEqual({
      type: 'cycle_session',
      dir: 1,
    });
    expect(resolveKeymap(ev({ key: 'K', metaKey: true }), terminalView)).toEqual({
      type: 'cycle_session',
      dir: -1,
    });
  });

  it('ターミナル画面でないときは Cmd+J/K を無視する（null を返す）', () => {
    expect(resolveKeymap(ev({ key: 'j', metaKey: true }), closed)).toBeNull();
    expect(resolveKeymap(ev({ key: 'k', metaKey: true }), closed)).toBeNull();
    expect(resolveKeymap(ev({ key: 'j', metaKey: true }), editorView)).toBeNull();
    expect(resolveKeymap(ev({ key: 'k', metaKey: true }), editorView)).toBeNull();
  });

  it('Cmd+N で新規セッションモーダルを開く', () => {
    expect(resolveKeymap(ev({ key: 'n', metaKey: true }), closed)).toEqual({
      type: 'open_create_session',
    });
  });

  it('Shift 併用で大文字になった N も受理する', () => {
    expect(resolveKeymap(ev({ key: 'N', metaKey: true }), closed)).toEqual({
      type: 'open_create_session',
    });
  });

  it('モーダルが開いていても Cmd+N は Cmd+N のまま扱う', () => {
    expect(resolveKeymap(ev({ key: 'n', metaKey: true }), open)).toEqual({
      type: 'open_create_session',
    });
  });

  it('Escape はモーダルが開いているときだけモーダルを閉じる', () => {
    expect(resolveKeymap(ev({ key: 'Escape', metaKey: false }), open)).toEqual({
      type: 'close_modal',
    });
    expect(resolveKeymap(ev({ key: 'Escape', metaKey: false }), closed)).toBeNull();
  });

  it('Cmd なしの 1 / n は何もしない（入力欄で文字が打てる）', () => {
    expect(resolveKeymap(ev({ key: '1', metaKey: false }), closed)).toBeNull();
    expect(resolveKeymap(ev({ key: 'n', metaKey: false }), closed)).toBeNull();
  });

  it('Cmd+P でプロジェクトスイッチャーを開閉する（契約 §11.4.2 の Cmd+P 行）', () => {
    expect(resolveKeymap(ev({ key: 'p', metaKey: true }), closed)).toEqual({
      type: 'toggle_project_switcher',
    });
  });

  // 契約 §11.4.2 の Cmd+P 行の view 条件は「無し」。3 画面すべてで同じアクションを返す。
  it('Cmd+P は kanban / terminal / editor のどの画面でも発火する（契約 §11.4.2: view 条件は無し）', () => {
    for (const ctx of [closed, terminalView, editorView]) {
      expect(resolveKeymap(ev({ key: 'p', metaKey: true }), ctx)).toEqual({
        type: 'toggle_project_switcher',
      });
    }
  });

  it('モーダル表示中でも Cmd+P は発火する（契約 §11.4.1 規則 M / §11.4.2 の「開いているモーダルを置き換える」）', () => {
    expect(resolveKeymap(ev({ key: 'p', metaKey: true }), open)).toEqual({
      type: 'toggle_project_switcher',
    });
  });

  it('Shift 併用で大文字になった P も受理する（契約 §97.2 規則 S）', () => {
    expect(resolveKeymap(ev({ key: 'P', metaKey: true }), closed)).toEqual({
      type: 'toggle_project_switcher',
    });
  });

  // 契約 §97.2 規則 C の 7 キー（Cmd+J / Cmd+K / Cmd+D / Cmd+[ / Cmd+] / Cmd+T / Cmd+W）に
  // Cmd+P は入っていない。§97.2 の表の `Cmd+N` / `Cmd+P` 行は Ctrl 併用「発火する」。
  it('Ctrl 併用の Cmd+P も発火する（契約 §97.2 規則 C の 7 キー集合に Cmd+P は入っていない）', () => {
    expect(resolveKeymap(ev({ key: 'p', metaKey: true, ctrlKey: true }), closed)).toEqual({
      type: 'toggle_project_switcher',
    });
  });

  it('Cmd なしの p は何もしない（ターミナル入力を奪わない）', () => {
    expect(resolveKeymap(ev({ key: 'p', metaKey: false }), closed)).toBeNull();
  });

  it('Cmd+3 で editor 画面へ切り替える（契約 §11 / §11.4.2: view 条件なし・モーダル表示中も発火）', () => {
    expect(resolveKeymap(ev({ key: '3', metaKey: true }), closed)).toEqual({
      type: 'set_view',
      view: 'editor',
    });
    // §11.4.2: Cmd+3 は view 条件を持たない
    expect(resolveKeymap(ev({ key: '3', metaKey: true }), terminalView)).toEqual({
      type: 'set_view',
      view: 'editor',
    });
    // §11.4.1 規則 M: モーダルが開いていても発火する
    expect(resolveKeymap(ev({ key: '3', metaKey: true }), open)).toEqual({
      type: 'set_view',
      view: 'editor',
    });
    // Cmd を伴わなければ null
    expect(resolveKeymap(ev({ key: '3', metaKey: false }), closed)).toBeNull();
  });

  // --- M3-2 Task 11: Cmd+D（3 値サイクル）/ Cmd+[ / Cmd+]（ペイン移動）---

  it('ターミナル画面では Cmd+D が toggleLayout', () => {
    expect(resolveKeymap(ev({ key: 'd', metaKey: true }), terminalView)).toEqual({
      type: 'toggle_layout',
    });
  });

  it('Shift 併用で大文字になった D も toggleLayout', () => {
    expect(resolveKeymap(ev({ key: 'D', metaKey: true }), terminalView)).toEqual({
      type: 'toggle_layout',
    });
  });

  it('ターミナル画面でないときは Cmd+D を無視する（契約 §11.4.2: view 条件は terminal のみ）', () => {
    expect(resolveKeymap(ev({ key: 'd', metaKey: true }), closed)).toBeNull();
    expect(resolveKeymap(ev({ key: 'd', metaKey: true }), editorView)).toBeNull();
  });

  it('Cmd なしの d は無視する（terminal でも vim の削除キーを奪わない）', () => {
    expect(resolveKeymap(ev({ key: 'd', metaKey: false }), terminalView)).toBeNull();
  });

  it('ターミナル画面では Cmd+[ で前のペイン（pane 0）、Cmd+] で次のペイン（pane 1）', () => {
    expect(resolveKeymap(ev({ key: '[', metaKey: true }), terminalView)).toEqual({
      type: 'set_active_pane',
      pane: 0,
    });
    expect(resolveKeymap(ev({ key: ']', metaKey: true }), terminalView)).toEqual({
      type: 'set_active_pane',
      pane: 1,
    });
  });

  it('ターミナル画面でないときは Cmd+[ / Cmd+] を無視する（契約 §11.4.2: view 条件は terminal のみ）', () => {
    expect(resolveKeymap(ev({ key: '[', metaKey: true }), closed)).toBeNull();
    expect(resolveKeymap(ev({ key: ']', metaKey: true }), closed)).toBeNull();
    expect(resolveKeymap(ev({ key: '[', metaKey: true }), editorView)).toBeNull();
    expect(resolveKeymap(ev({ key: ']', metaKey: true }), editorView)).toBeNull();
  });

  it('Cmd なしの [ は無視する（terminal でも通常入力を奪わない）', () => {
    expect(resolveKeymap(ev({ key: '[', metaKey: false }), terminalView)).toBeNull();
  });

  it('WKWebView に Cmd+[/] が奪われた場合のフォールバック Cmd+Alt+←/→ も同じアクション', () => {
    expect(
      resolveKeymap(ev({ key: 'ArrowLeft', metaKey: true, altKey: true }), terminalView),
    ).toEqual({
      type: 'set_active_pane',
      pane: 0,
    });
    expect(
      resolveKeymap(ev({ key: 'ArrowRight', metaKey: true, altKey: true }), terminalView),
    ).toEqual({
      type: 'set_active_pane',
      pane: 1,
    });
  });

  it('ターミナル画面でないときは Cmd+Alt+←/→ フォールバックも無視する', () => {
    expect(resolveKeymap(ev({ key: 'ArrowLeft', metaKey: true, altKey: true }), closed)).toBeNull();
    expect(
      resolveKeymap(ev({ key: 'ArrowLeft', metaKey: true, altKey: true }), editorView),
    ).toBeNull();
  });

  it('モーダル表示中でも terminal 画面なら Cmd+D / Cmd+[ / Cmd+J / Cmd+K は発火する（契約 §11.4.1 規則 M）', () => {
    expect(resolveKeymap(ev({ key: 'd', metaKey: true }), terminalModal)).toEqual({
      type: 'toggle_layout',
    });
    expect(resolveKeymap(ev({ key: '[', metaKey: true }), terminalModal)).toEqual({
      type: 'set_active_pane',
      pane: 0,
    });
    expect(resolveKeymap(ev({ key: 'j', metaKey: true }), terminalModal)).toEqual({
      type: 'cycle_session',
      dir: 1,
    });
    expect(resolveKeymap(ev({ key: 'k', metaKey: true }), terminalModal)).toEqual({
      type: 'cycle_session',
      dir: -1,
    });
  });

  it('Ctrl 併用の Cmd+D / Cmd+[ / Cmd+] は無視する（アプリが消費しない。契約 §97.2）', () => {
    expect(resolveKeymap(ev({ key: 'd', metaKey: true, ctrlKey: true }), terminalView)).toBeNull();
    expect(resolveKeymap(ev({ key: '[', metaKey: true, ctrlKey: true }), terminalView)).toBeNull();
    expect(resolveKeymap(ev({ key: ']', metaKey: true, ctrlKey: true }), terminalView)).toBeNull();
  });

  it('Ctrl 併用の Cmd+J / Cmd+K は無視する（アプリが消費しない。契約 §97.2。brief Step 1 指定）', () => {
    expect(resolveKeymap(ev({ key: 'j', metaKey: true, ctrlKey: true }), terminalView)).toBeNull();
    expect(resolveKeymap(ev({ key: 'k', metaKey: true, ctrlKey: true }), terminalView)).toBeNull();
  });

  it('Alt 併用の [ は無視する（矢印以外の Alt 組合せを拾わない）', () => {
    expect(resolveKeymap(ev({ key: '[', metaKey: true, altKey: true }), terminalView)).toBeNull();
  });

  it('未知の矢印（Alt なし）は無視する', () => {
    expect(resolveKeymap(ev({ key: 'ArrowLeft', metaKey: true }), terminalView)).toBeNull();
  });

  // --- M3-4 Task 20: Cmd+T（スクラッチ新規作成）/ Cmd+W（スクラッチを閉じる）契約 §29.8 ---

  it('ターミナル画面では Cmd+T が create_scratch_terminal', () => {
    expect(resolveKeymap(ev({ key: 't', metaKey: true }), terminalView)).toEqual({
      type: 'create_scratch_terminal',
    });
  });

  it('ターミナル画面では Cmd+W が close_scratch_terminal', () => {
    expect(resolveKeymap(ev({ key: 'w', metaKey: true }), terminalView)).toEqual({
      type: 'close_scratch_terminal',
    });
  });

  it('ターミナル画面でないときは Cmd+T / Cmd+W を無視する（契約 §11.4.2: view 条件は terminal のみ）', () => {
    expect(resolveKeymap(ev({ key: 't', metaKey: true }), closed)).toBeNull();
    expect(resolveKeymap(ev({ key: 'w', metaKey: true }), closed)).toBeNull();
    expect(resolveKeymap(ev({ key: 't', metaKey: true }), editorView)).toBeNull();
    expect(resolveKeymap(ev({ key: 'w', metaKey: true }), editorView)).toBeNull();
  });

  it('Ctrl 併用の Cmd+T / Cmd+W は無視する（契約 §97.2 規則 C の 7 キー集合に含まれる）', () => {
    expect(resolveKeymap(ev({ key: 't', metaKey: true, ctrlKey: true }), terminalView)).toBeNull();
    expect(resolveKeymap(ev({ key: 'w', metaKey: true, ctrlKey: true }), terminalView)).toBeNull();
  });

  it('Alt 併用の Cmd+T / Cmd+W は無視する（契約 §97.2 規則 A。Cmd+D と同じ扱い）', () => {
    expect(resolveKeymap(ev({ key: 't', metaKey: true, altKey: true }), terminalView)).toBeNull();
    expect(resolveKeymap(ev({ key: 'w', metaKey: true, altKey: true }), terminalView)).toBeNull();
  });

  it('Shift 併用で大文字になった T / W も受理する（契約 §97.2 規則 S）', () => {
    expect(resolveKeymap(ev({ key: 'T', metaKey: true }), terminalView)).toEqual({
      type: 'create_scratch_terminal',
    });
    expect(resolveKeymap(ev({ key: 'W', metaKey: true }), terminalView)).toEqual({
      type: 'close_scratch_terminal',
    });
  });

  it('モーダル表示中でも terminal 画面なら Cmd+T / Cmd+W は発火する（契約 §11.4.1 規則 M）', () => {
    expect(resolveKeymap(ev({ key: 't', metaKey: true }), terminalModal)).toEqual({
      type: 'create_scratch_terminal',
    });
    expect(resolveKeymap(ev({ key: 'w', metaKey: true }), terminalModal)).toEqual({
      type: 'close_scratch_terminal',
    });
  });

  it('Cmd なしの t / w は無視する（terminal でも通常入力を奪わない）', () => {
    expect(resolveKeymap(ev({ key: 't', metaKey: false }), terminalView)).toBeNull();
    expect(resolveKeymap(ev({ key: 'w', metaKey: false }), terminalView)).toBeNull();
  });

  // 契約 §103.6（§87.2 の形 1）: macOS では Alt が event.key そのものを書き換えるため、
  // Cmd+Alt+J は 'j' の分岐に到達しない。KeymapEvent が code を持たないので
  // key: '∆' だけで書く（§103.6.1: 穴を塞ぐために型へ code を足さない）。
  it('Cmd+Alt+J は event.key が書き換わるため cycle_session を返さない（契約 §103.6）', () => {
    expect(resolveKeymap(ev({ key: '∆', metaKey: true, altKey: true }), terminalView)).toBeNull();
  });
});
