import { describe, expect, it } from 'vitest';
import { visibleSessionIds, type VisibilityInput } from './visibility';

const base: VisibilityInput = {
  view: 'terminal',
  layout: 'single',
  paneAssignment: [null, null],
  focusedSessionId: null,
};

describe('visibleSessionIds', () => {
  it('カンバン画面では何も表示していない扱いになる', () => {
    expect(visibleSessionIds({ ...base, view: 'kanban', focusedSessionId: 's1' })).toEqual([]);
  });

  it('エディタ画面では agent の出力が見えないので空になる', () => {
    expect(visibleSessionIds({ ...base, view: 'editor', focusedSessionId: 's1' })).toEqual([]);
  });

  it('1面表示ではフォーカス中セッションだけが見えている', () => {
    expect(visibleSessionIds({ ...base, focusedSessionId: 's1' })).toEqual(['s1']);
  });

  it('1面表示でフォーカスが無ければ空', () => {
    expect(visibleSessionIds(base)).toEqual([]);
  });

  it('2分割では両ペインのセッションが見えている', () => {
    expect(
      visibleSessionIds({ ...base, layout: 'split2', paneAssignment: ['s1', 's2'] }),
    ).toEqual(['s1', 's2']);
  });

  it('2分割で片方が未割り当てなら割り当て済みだけを返す', () => {
    expect(
      visibleSessionIds({ ...base, layout: 'split2', paneAssignment: ['s1', null] }),
    ).toEqual(['s1']);
  });

  it('2分割で同じセッションが両ペインにあっても重複しない', () => {
    expect(
      visibleSessionIds({ ...base, layout: 'split2', paneAssignment: ['s1', 's1'] }),
    ).toEqual(['s1']);
  });

  it('縦分割(split2-v)でも両ペインのセッションが見えている', () => {
    expect(
      visibleSessionIds({ ...base, layout: 'split2-v', paneAssignment: ['s1', 's2'] }),
    ).toEqual(['s1', 's2']);
  });
});
