import { describe, it, expect } from 'vitest';
import { otherPane, visiblePanes, assignPaneReducer, type PaneState } from './paneLogic';

const S = (
  layout: PaneState['layout'],
  paneAssignment: PaneState['paneAssignment'],
  activePane: PaneState['activePane'],
): PaneState => ({ layout, paneAssignment, activePane });

describe('otherPane', () => {
  it('0 と 1 を入れ替える', () => {
    expect(otherPane(0)).toBe(1);
    expect(otherPane(1)).toBe(0);
  });
});

describe('visiblePanes', () => {
  it('single では activePane のみ', () => {
    expect(visiblePanes(S('single', ['a', 'b'], 0))).toEqual([0]);
    expect(visiblePanes(S('single', ['a', 'b'], 1))).toEqual([1]);
  });

  it('split2 では両方を左から', () => {
    expect(visiblePanes(S('split2', ['a', 'b'], 1))).toEqual([0, 1]);
  });
});

describe('assignPaneReducer', () => {
  it('空きペインに割り当て、そのペインをアクティブにする', () => {
    const next = assignPaneReducer(S('split2', ['a', null], 0), 1, 'b');
    expect(next.paneAssignment).toEqual(['a', 'b']);
    expect(next.activePane).toBe(1);
  });

  it('既に割り当て済みのペインは置き換える', () => {
    const next = assignPaneReducer(S('split2', ['a', 'b'], 0), 1, 'c');
    expect(next.paneAssignment).toEqual(['a', 'c']);
    expect(next.activePane).toBe(1);
  });

  it('もう一方のペインに居るセッションを要求されたらスワップする', () => {
    const next = assignPaneReducer(S('split2', ['a', 'b'], 1), 0, 'b');
    expect(next.paneAssignment).toEqual(['b', 'a']);
    expect(next.activePane).toBe(0);
  });

  it('スワップ後も同一セッションが両ペインに存在しない', () => {
    const next = assignPaneReducer(S('split2', ['a', 'b'], 0), 0, 'b');
    expect(next.paneAssignment[0]).not.toBe(next.paneAssignment[1]);
  });

  it('既にそのペインに居る場合は割当を変えず activePane だけ揃える', () => {
    const before = S('split2', ['a', 'b'], 1);
    const next = assignPaneReducer(before, 0, 'a');
    expect(next.paneAssignment).toEqual(['a', 'b']);
    expect(next.activePane).toBe(0);
  });

  it('layout は変更しない', () => {
    expect(assignPaneReducer(S('single', [null, null], 0), 0, 'a').layout).toBe('single');
  });

  it('single では pane 引数を無視して表示中のペインに割り当てる', () => {
    const next = assignPaneReducer(S('single', ['a', null], 0), 1, 'b');
    expect(next.paneAssignment).toEqual(['b', null]);
    expect(next.activePane).toBe(0);
  });
});
