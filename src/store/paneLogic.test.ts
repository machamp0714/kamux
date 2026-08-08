import { describe, it, expect } from 'vitest';
import {
  otherPane,
  visiblePanes,
  assignPaneReducer,
  setLayoutReducer,
  setActivePaneReducer,
  type PaneState,
} from './paneLogic';

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

describe('setLayoutReducer', () => {
  it('paneAssignment と activePane を一切変更しない', () => {
    const before = S('split2', ['a', 'b'], 1);
    const next = setLayoutReducer(before, 'single');
    expect(next.layout).toBe('single');
    expect(next.paneAssignment).toEqual(['a', 'b']);
    expect(next.paneAssignment).toBe(before.paneAssignment);
    expect(next.activePane).toBe(1);
  });

  it('split2 に戻すと左右が元の位置のまま復帰する', () => {
    const start = S('split2', ['a', 'b'], 1);
    const round = setLayoutReducer(setLayoutReducer(start, 'single'), 'split2');
    expect(round.layout).toBe('split2');
    expect(round.paneAssignment).toEqual(['a', 'b']);
    expect(round.activePane).toBe(1);
  });

  it('同じ layout なら同一オブジェクトを返す', () => {
    const before = S('single', ['a', null], 0);
    expect(setLayoutReducer(before, 'single')).toBe(before);
  });
});

describe('setActivePaneReducer', () => {
  it('split2 ではアクティブペインを移す', () => {
    const before = S('split2', ['a', 'b'], 0);
    const next = setActivePaneReducer(before, 1);
    expect(next.activePane).toBe(1);
    expect(next.paneAssignment).toEqual(['a', 'b']);
    expect(next.paneAssignment).toBe(before.paneAssignment);

    // isSplit() 経由であることの証跡: split2-v でも同じ結果になる（契約 §28.2）。
    const beforeV = S('split2-v', ['a', 'b'], 0);
    const nextV = setActivePaneReducer(beforeV, 1);
    expect(nextV.activePane).toBe(1);
    expect(nextV.paneAssignment).toEqual(['a', 'b']);
    expect(nextV.paneAssignment).toBe(beforeV.paneAssignment);
  });

  it('single では no-op（同一オブジェクトを返す）', () => {
    const before = S('single', ['a', 'b'], 0);
    expect(setActivePaneReducer(before, 1)).toBe(before);
  });

  it('同じペインなら同一オブジェクトを返す', () => {
    const before = S('split2', ['a', 'b'], 1);
    expect(setActivePaneReducer(before, 1)).toBe(before);
  });
});
