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

  it('M3-4 の未実装キー（Cmd+P）は null を返す', () => {
    // 契約 §11 のうち M3-4 の担当外。後続フェーズがこのユニオンに variant を足す
    expect(resolveKeymap(ev({ key: 'p', metaKey: true }), closed)).toBeNull();
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

  it('モーダル表示中でも terminal 画面なら Cmd+D / Cmd+[ は発火する（契約 §11.4.1 規則 M）', () => {
    expect(resolveKeymap(ev({ key: 'd', metaKey: true }), terminalModal)).toEqual({
      type: 'toggle_layout',
    });
    expect(resolveKeymap(ev({ key: '[', metaKey: true }), terminalModal)).toEqual({
      type: 'set_active_pane',
      pane: 0,
    });
  });

  it('Ctrl 併用の Cmd+D / Cmd+[ / Cmd+] は無視する（ターミナルへ通す）', () => {
    expect(resolveKeymap(ev({ key: 'd', metaKey: true, ctrlKey: true }), terminalView)).toBeNull();
    expect(resolveKeymap(ev({ key: '[', metaKey: true, ctrlKey: true }), terminalView)).toBeNull();
    expect(resolveKeymap(ev({ key: ']', metaKey: true, ctrlKey: true }), terminalView)).toBeNull();
  });

  it('Ctrl 併用の Cmd+J / Cmd+K は無視する（ターミナルへ通す。brief Step 1 指定）', () => {
    expect(resolveKeymap(ev({ key: 'j', metaKey: true, ctrlKey: true }), terminalView)).toBeNull();
    expect(resolveKeymap(ev({ key: 'k', metaKey: true, ctrlKey: true }), terminalView)).toBeNull();
  });

  it('Alt 併用の [ は無視する（矢印以外の Alt 組合せを拾わない）', () => {
    expect(resolveKeymap(ev({ key: '[', metaKey: true, altKey: true }), terminalView)).toBeNull();
  });

  it('未知の矢印（Alt なし）は無視する', () => {
    expect(resolveKeymap(ev({ key: 'ArrowLeft', metaKey: true }), terminalView)).toBeNull();
  });
});
