import { describe, expect, it } from 'vitest';
import { resolveKeymap } from './keymap';

const closed = { modalOpen: false };
const open = { modalOpen: true };

describe('resolveKeymap', () => {
  it('Cmd+1 でカンバン画面へ切り替える（契約 §11）', () => {
    expect(resolveKeymap({ key: '1', metaKey: true }, closed)).toEqual({
      type: 'set_view',
      view: 'kanban',
    });
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

  it('M1-2 で未実装のキーは null を返す', () => {
    // 契約 §11 のうち M1-2 の担当外。後続フェーズがこのユニオンに variant を足す
    for (const key of ['2', '3', 'p', 'j', 'k', 'd', '[', ']']) {
      expect(resolveKeymap({ key, metaKey: true }, closed)).toBeNull();
    }
  });
});
