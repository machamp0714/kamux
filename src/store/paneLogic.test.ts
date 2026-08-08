import { describe, it, expect } from 'vitest';
import {
  otherPane,
  visiblePanes,
  assignPaneReducer,
  setLayoutReducer,
  setActivePaneReducer,
  nextSessionId,
  cycleSessionReducer,
  routeFocusReducer,
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

describe('nextSessionId', () => {
  const order = ['a', 'b', 'c'];

  it('dir=1 で次へ進む', () => {
    expect(nextSessionId(order, 'a', 1, [])).toBe('b');
  });

  it('dir=-1 で前へ戻る', () => {
    expect(nextSessionId(order, 'b', -1, [])).toBe('a');
  });

  it('末尾から dir=1 で先頭へ巡回する', () => {
    expect(nextSessionId(order, 'c', 1, [])).toBe('a');
  });

  it('先頭から dir=-1 で末尾へ巡回する', () => {
    expect(nextSessionId(order, 'a', -1, [])).toBe('c');
  });

  it('除外されたセッションを飛ばす', () => {
    expect(nextSessionId(order, 'a', 1, ['b'])).toBe('c');
    // current より前を除外すると order 上の index と candidates 上の index がずれる。
    // ここで candidates 側の index を使わないと 'c'（自分自身）や 'a' を誤って返す。
    expect(nextSessionId(order, 'c', 1, ['a'])).toBe('b');
  });

  it('current が null なら dir=1 で先頭を返す', () => {
    expect(nextSessionId(order, null, 1, [])).toBe('a');
  });

  it('current が null なら dir=-1 で末尾を返す', () => {
    expect(nextSessionId(order, null, -1, [])).toBe('c');
  });

  it('current が order に無ければ先頭/末尾にフォールバックする', () => {
    expect(nextSessionId(order, 'zzz', 1, [])).toBe('a');
    expect(nextSessionId(order, 'zzz', -1, [])).toBe('c');
  });

  it('候補が空なら null', () => {
    expect(nextSessionId([], 'a', 1, [])).toBeNull();
    expect(nextSessionId(['a'], 'a', 1, ['a'])).toBeNull();
  });

  it('候補が current 1 件だけなら自分自身を返す', () => {
    expect(nextSessionId(order, 'a', 1, ['b', 'c'])).toBe('a');
  });
});

describe('cycleSessionReducer', () => {
  const order = ['a', 'b', 'c', 'd'];

  it('single では activePane のセッションを素直に巡回する', () => {
    const next = cycleSessionReducer(S('single', ['a', null], 0), order, 1);
    expect(next.paneAssignment).toEqual(['b', null]);
    expect(next.activePane).toBe(0);
    expect(next.layout).toBe('single');
  });

  it('split2 ではもう一方のペインのセッションをスキップする', () => {
    // 左=a / 右=b、左で Cmd+J → b を飛ばして c
    const next = cycleSessionReducer(S('split2', ['a', 'b'], 0), order, 1);
    expect(next.paneAssignment).toEqual(['c', 'b']);

    // isSplit() 経由であることの証跡: split2-v でも同じ結果になる（契約 §28.2）。
    // isSplit(s.layout) を s.layout === 'split2' に退化させると、この assert が
    // 独り赤くなる（'split2-v' は isSplit=true だが個別比較では false になり
    // もう一方のペインのスキップが効かなくなる）。
    const nextV = cycleSessionReducer(S('split2-v', ['a', 'b'], 0), order, 1);
    expect(nextV.paneAssignment).toEqual(['c', 'b']);
  });

  it('split2 の dir=-1 でももう一方をスキップする', () => {
    // 左=c / 右=b、左で Cmd+K → b を飛ばして a
    const next = cycleSessionReducer(S('split2', ['c', 'b'], 0), order, -1);
    expect(next.paneAssignment).toEqual(['a', 'b']);
  });

  it('single では裏スロットのセッションを除外しない（到達不能にしない）', () => {
    // 表示は左の a のみ、裏スロットに b が退避している
    const next = cycleSessionReducer(S('single', ['a', 'b'], 0), order, 1);
    expect(next.paneAssignment[0]).toBe('b');
  });

  it('single で裏スロットのセッションへ巡回するとスワップになり、両スロットが同じにならない', () => {
    const next = cycleSessionReducer(S('single', ['a', 'b'], 0), order, 1);
    expect(next.paneAssignment).toEqual(['b', 'a']);
    expect(next.paneAssignment[0]).not.toBe(next.paneAssignment[1]);
  });

  it('split2 で候補がもう一方の 1 件しかなければ何もしない', () => {
    const before = S('split2', ['a', 'b'], 0);
    expect(cycleSessionReducer(before, ['a', 'b'], 1)).toBe(before);
  });

  it('タブが空なら何もしない', () => {
    const before = S('single', [null, null], 0);
    expect(cycleSessionReducer(before, [], 1)).toBe(before);
  });

  it('割当が null の状態から先頭のセッションを掴む', () => {
    const next = cycleSessionReducer(S('single', [null, null], 0), order, 1);
    expect(next.paneAssignment).toEqual(['a', null]);
  });

  it('activePane が 1 のときは右ペインだけを動かす', () => {
    const next = cycleSessionReducer(S('split2', ['a', 'b'], 1), order, 1);
    expect(next.paneAssignment).toEqual(['a', 'c']);
    expect(next.activePane).toBe(1);
  });
});

describe('routeFocusReducer', () => {
  it('既にアクティブペインに居るなら何もしない', () => {
    const before = S('split2', ['a', 'b'], 0);
    expect(routeFocusReducer(before, 'a')).toBe(before);
  });

  it('split2 でもう一方のペインに居るなら activePane を移すだけ（割当は動かさない）', () => {
    const next = routeFocusReducer(S('split2', ['a', 'b'], 0), 'b');
    expect(next.paneAssignment).toEqual(['a', 'b']);
    expect(next.activePane).toBe(1);

    // isSplit() 経由であることの証跡: split2-v でも同じ結果になる（契約 §28.2 追跡表 #5）。
    // isSplit(s.layout) を s.layout === 'split2' に退化させると、split2-v では
    // もう一方のペインへのルーティングが効かず assignPaneReducer 側へ落ちてしまい、
    // このアサートだけが独り赤くなる。
    const nextV = routeFocusReducer(S('split2-v', ['a', 'b'], 0), 'b');
    expect(nextV.paneAssignment).toEqual(['a', 'b']);
    expect(nextV.activePane).toBe(1);
  });

  it('どこにも居ないならアクティブペインに割り当てる', () => {
    const next = routeFocusReducer(S('split2', ['a', 'b'], 1), 'c');
    expect(next.paneAssignment).toEqual(['a', 'c']);
    expect(next.activePane).toBe(1);
  });

  it('single で裏スロットに居る場合はアクティブペインに引き込む（スワップ）', () => {
    // single では裏スロットは見えないので activePane を移してはならない
    const next = routeFocusReducer(S('single', ['a', 'b'], 0), 'b');
    expect(next.activePane).toBe(0);
    expect(next.paneAssignment).toEqual(['b', 'a']);
  });

  it('layout は変更しない', () => {
    expect(routeFocusReducer(S('single', ['a', null], 0), 'c').layout).toBe('single');
  });
});
