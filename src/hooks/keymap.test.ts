import { describe, expect, it } from 'vitest';
import { resolveKeymap } from './keymap';

const closed = { modalOpen: false, view: 'kanban' as const };
const open = { modalOpen: true, view: 'kanban' as const };
const terminalView = { modalOpen: false, view: 'terminal' as const };

describe('resolveKeymap', () => {
  it('Cmd+1 でカンバン画面へ切り替える（契約 §11）', () => {
    expect(resolveKeymap({ key: '1', metaKey: true }, closed)).toEqual({
      type: 'set_view',
      view: 'kanban',
    });
  });

  it('Cmd+2 でターミナル画面へ切り替える（契約 §11）', () => {
    expect(resolveKeymap({ key: '2', metaKey: true }, closed)).toEqual({
      type: 'set_view',
      view: 'terminal',
    });
  });

  it('ターミナル画面では Cmd+J が cycleSession(1)', () => {
    expect(resolveKeymap({ key: 'j', metaKey: true }, terminalView)).toEqual({
      type: 'cycle_session',
      dir: 1,
    });
  });

  it('ターミナル画面では Cmd+K が cycleSession(-1)', () => {
    expect(resolveKeymap({ key: 'k', metaKey: true }, terminalView)).toEqual({
      type: 'cycle_session',
      dir: -1,
    });
  });

  it('Shift 併用で大文字になっても Cmd+J/K は効く', () => {
    expect(resolveKeymap({ key: 'J', metaKey: true }, terminalView)).toEqual({
      type: 'cycle_session',
      dir: 1,
    });
    expect(resolveKeymap({ key: 'K', metaKey: true }, terminalView)).toEqual({
      type: 'cycle_session',
      dir: -1,
    });
  });

  it('ターミナル画面でないときは Cmd+J/K を無視する（null を返す）', () => {
    expect(resolveKeymap({ key: 'j', metaKey: true }, closed)).toBeNull();
    expect(resolveKeymap({ key: 'k', metaKey: true }, closed)).toBeNull();
  });

  it('Cmd+N で新規セッションモーダルを開く', () => {
    expect(resolveKeymap({ key: 'n', metaKey: true }, closed)).toEqual({
      type: 'open_create_session',
    });
  });

  it('Shift 併用で大文字になった N も受理する', () => {
    expect(resolveKeymap({ key: 'N', metaKey: true }, closed)).toEqual({
      type: 'open_create_session',
    });
  });

  it('モーダルが開いていても Cmd+N は Cmd+N のまま扱う', () => {
    expect(resolveKeymap({ key: 'n', metaKey: true }, open)).toEqual({
      type: 'open_create_session',
    });
  });

  it('Escape はモーダルが開いているときだけモーダルを閉じる', () => {
    expect(resolveKeymap({ key: 'Escape', metaKey: false }, open)).toEqual({
      type: 'close_modal',
    });
    expect(resolveKeymap({ key: 'Escape', metaKey: false }, closed)).toBeNull();
  });

  it('Cmd なしの 1 / n は何もしない（入力欄で文字が打てる）', () => {
    expect(resolveKeymap({ key: '1', metaKey: false }, closed)).toBeNull();
    expect(resolveKeymap({ key: 'n', metaKey: false }, closed)).toBeNull();
  });

  it('M3 系の未実装キーは null を返す', () => {
    // 契約 §11 のうち M3-2 / M3-4 の担当外。後続フェーズがこのユニオンに variant を足す
    for (const key of ['p', 'd', '[', ']']) {
      expect(resolveKeymap({ key, metaKey: true }, closed)).toBeNull();
    }
  });

  it('Cmd+3 で editor 画面へ切り替える（契約 §11 / §11.4.2: view 条件なし・モーダル表示中も発火）', () => {
    expect(resolveKeymap({ key: '3', metaKey: true }, closed)).toEqual({
      type: 'set_view',
      view: 'editor',
    });
    // §11.4.2: Cmd+3 は view 条件を持たない
    expect(resolveKeymap({ key: '3', metaKey: true }, terminalView)).toEqual({
      type: 'set_view',
      view: 'editor',
    });
    // §11.4.1 規則 M: モーダルが開いていても発火する
    expect(resolveKeymap({ key: '3', metaKey: true }, open)).toEqual({
      type: 'set_view',
      view: 'editor',
    });
    // Cmd を伴わなければ null
    expect(resolveKeymap({ key: '3', metaKey: false }, closed)).toBeNull();
  });
});
