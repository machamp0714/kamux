import { describe, it, expect } from 'vitest';
import {
  isSplit,
  nextLayout,
  isLayout,
  otherPane,
  visiblePanes,
  assignPaneReducer,
  setLayoutReducer,
  setActivePaneReducer,
  nextSessionId,
  cycleSessionReducer,
  routeFocusReducer,
  visibleAgentSurfaces,
  surfacesToDetach,
  paneBadgeFor,
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

    // isSplit() 経由であることの証跡（契約 §28.2）。isSplit(s.layout) を
    // s.layout === 'split2' に退化させると、split2-v がここだけ独り赤くなる。
    expect(visiblePanes(S('split2-v', ['a', 'b'], 1))).toEqual([0, 1]);
  });
});

describe('assignPaneReducer', () => {
  it('空きペインに割り当て、そのペインをアクティブにする', () => {
    const next = assignPaneReducer(S('split2', ['a', null], 0), 1, 'b');
    expect(next.paneAssignment).toEqual(['a', 'b']);
    expect(next.activePane).toBe(1);

    // isSplit() 経由であることの証跡（契約 §28.2）。isSplit(s.layout) を
    // s.layout === 'split2' に退化させると、split2-v がここだけ独り赤くなる。
    const nextV = assignPaneReducer(S('split2-v', ['a', null], 0), 1, 'b');
    expect(nextV.paneAssignment).toEqual(['a', 'b']);
    expect(nextV.activePane).toBe(1);
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

    // target: 1 の反対方向。この早期リターン分岐（既にそのペインに居る）は
    // target をそのまま返さず固定値へすり替える退化を、片方向だけでは検出できない
    // （棚卸しで発見。M8 と同形）。
    const beforeR = S('split2', ['a', 'b'], 0);
    const nextR = assignPaneReducer(beforeR, 1, 'b');
    expect(nextR.paneAssignment).toEqual(['a', 'b']);
    expect(nextR.activePane).toBe(1);
  });

  it('layout は変更しない', () => {
    expect(assignPaneReducer(S('single', [null, null], 0), 0, 'a').layout).toBe('single');
  });

  it('single では pane 引数を無視して表示中のペインに割り当てる', () => {
    const next = assignPaneReducer(S('single', ['a', null], 0), 1, 'b');
    expect(next.paneAssignment).toEqual(['b', null]);
    expect(next.activePane).toBe(0);

    // 不変条件 4（single の間 activePane は変化しない）の反対方向。
    // activePane: 1 起点が無いと、target を `s.activePane` ではなく
    // 固定値 `0` に退化させても（PR 25 レビュー M8）、上のケースは
    // たまたま activePane が既に 0 なので気づけない。
    const nextV = assignPaneReducer(S('single', [null, 'a'], 1), 0, 'b');
    expect(nextV.paneAssignment).toEqual([null, 'b']);
    expect(nextV.activePane).toBe(1);
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

    // activePane: 0 起点の反対方向。s.activePane をそのまま返さず固定値へ
    // すり替える退化を、片方向だけでは検出できない（棚卸しで発見。M8 と同形）。
    const beforeZ = S('split2', ['a', 'b'], 0);
    const nextZ = setLayoutReducer(beforeZ, 'single');
    expect(nextZ.activePane).toBe(0);
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

    // activePane: 1 → pane: 0 の反対方向。pane をそのまま返さず固定値へ
    // すり替える退化を、片方向だけでは検出できない（棚卸しで発見。M8 と同形）。
    const beforeR = S('split2', ['a', 'b'], 1);
    const nextR = setActivePaneReducer(beforeR, 0);
    expect(nextR.activePane).toBe(0);
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
    const before = S('split2', ['a', 'b'], 0);
    const next = routeFocusReducer(before, 'b');
    expect(next.paneAssignment).toEqual(['a', 'b']);
    // 「割当は動かさない」の実体: 新しい配列を作らず同一参照を返す
    // （契約 §81.2。paneAssignment を [s.paneAssignment[0], s.paneAssignment[1]] のような
    // 新しい配列にすり替えても toEqual は緑のままになるため toBe で参照を見る）。
    expect(next.paneAssignment).toBe(before.paneAssignment);
    expect(next.activePane).toBe(1);

    // isSplit() 経由であることの証跡: split2-v でも同じ結果になる（契約 §28.2 追跡表 #5）。
    // isSplit(s.layout) を s.layout === 'split2' に退化させると、split2-v では
    // もう一方のペインへのルーティングが効かず assignPaneReducer 側へ落ちてしまい、
    // このアサートだけが独り赤くなる。
    const beforeV = S('split2-v', ['a', 'b'], 0);
    const nextV = routeFocusReducer(beforeV, 'b');
    expect(nextV.paneAssignment).toEqual(['a', 'b']);
    expect(nextV.activePane).toBe(1);
    // split 分岐の返り値は layout を保持する（isSplit を通さず 'split2' に固定しても
    // split2-v の他のアサートは緑のままになるため layout 自体を見る）。
    expect(nextV.layout).toBe('split2-v');
  });

  it('split2 で activePane が 1 のときにもう一方のペインへ移すと 0 に戻る', () => {
    // 上の split2 テストは activePane: 0 → 1 の 1 方向しか通らないため、
    // otherPane(1) === 0 の経路がここまで一度も観測されていなかった。
    const before = S('split2', ['a', 'b'], 1);
    const next = routeFocusReducer(before, 'a');
    expect(next.paneAssignment).toBe(before.paneAssignment);
    expect(next.activePane).toBe(0);
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

// 契約 §28.2 / §28.5 / §28.6。レイアウト比較の書き漏らしを機械的に検出する唯一の防具
// （計画 M3-2-split-layout.md Task 8 Step 1 から前倒し。PR 25 レビュー Important #4）
describe('isSplit / nextLayout / isLayout', () => {
  it('single だけが非分割', () => {
    expect(isSplit('single')).toBe(false);
    expect(isSplit('split2')).toBe(true);
    expect(isSplit('split2-v')).toBe(true);
  });

  it('nextLayout は single → split2 → split2-v → single を一周する', () => {
    expect(nextLayout('single')).toBe('split2');
    expect(nextLayout('split2')).toBe('split2-v');
    expect(nextLayout('split2-v')).toBe('single');
  });

  it('nextLayout を 3 回適用すると元に戻る', () => {
    for (const l of ['single', 'split2', 'split2-v'] as const) {
      expect(nextLayout(nextLayout(nextLayout(l)))).toBe(l);
    }
  });

  it('isLayout は既知の 3 値だけを通す', () => {
    expect(isLayout('single')).toBe(true);
    expect(isLayout('split2')).toBe(true);
    expect(isLayout('split2-v')).toBe(true);
    expect(isLayout('split2v')).toBe(false);
    expect(isLayout('vsplit')).toBe(false);
    expect(isLayout(undefined)).toBe(false);
    expect(isLayout(null)).toBe(false);
  });
});

describe('visibleAgentSurfaces', () => {
  it('split2 では両ペインの agent サーフェスを左から返す', () => {
    expect(visibleAgentSurfaces(S('split2', ['a', 'b'], 0))).toEqual(['a:agent', 'b:agent']);
  });

  it('single では表示中のペインだけを返す（裏スロットを含めない）', () => {
    expect(visibleAgentSurfaces(S('single', ['a', 'b'], 0))).toEqual(['a:agent']);
    expect(visibleAgentSurfaces(S('single', ['a', 'b'], 1))).toEqual(['b:agent']);
  });

  it('未割当のペインを飛ばす', () => {
    expect(visibleAgentSurfaces(S('split2', ['a', null], 0))).toEqual(['a:agent']);
    expect(visibleAgentSurfaces(S('single', [null, null], 0))).toEqual([]);
  });

  it('editor サーフェスは決して含めない', () => {
    expect(visibleAgentSurfaces(S('split2', ['a', 'b'], 0)).join()).not.toContain('editor');
  });
});

describe('surfacesToDetach', () => {
  it('表示集合から外れたものだけを返す', () => {
    expect(surfacesToDetach(['a:agent', 'b:agent'], ['a:agent'])).toEqual(['b:agent']);
  });

  it('左右スワップでは detach が発生しない（集合が同じ）', () => {
    expect(surfacesToDetach(['a:agent', 'b:agent'], ['b:agent', 'a:agent'])).toEqual([]);
  });

  it('初回描画では detach が発生しない', () => {
    expect(surfacesToDetach([], ['a:agent'])).toEqual([]);
  });

  it('アンマウント時は全件を返す', () => {
    expect(surfacesToDetach(['a:agent', 'b:agent'], [])).toEqual(['a:agent', 'b:agent']);
  });
});

describe('paneBadgeFor', () => {
  it('split2 で左ペインのセッションに L', () => {
    expect(paneBadgeFor(S('split2', ['a', 'b'], 0), 'a')).toBe('L');
  });

  it('split2 で右ペインのセッションに R', () => {
    expect(paneBadgeFor(S('split2', ['a', 'b'], 0), 'b')).toBe('R');
  });

  it('どちらにも出ていなければ null', () => {
    expect(paneBadgeFor(S('split2', ['a', 'b'], 0), 'c')).toBeNull();
  });

  it('single では常に null（ペインの概念を見せない）', () => {
    expect(paneBadgeFor(S('single', ['a', 'b'], 0), 'a')).toBeNull();
    expect(paneBadgeFor(S('single', ['a', 'b'], 0), 'b')).toBeNull();
  });

  // 契約 §28.3
  it('split2-v で上ペインのセッションに U', () => {
    expect(paneBadgeFor(S('split2-v', ['a', 'b'], 0), 'a')).toBe('U');
  });

  it('split2-v で下ペインのセッションに D', () => {
    expect(paneBadgeFor(S('split2-v', ['a', 'b'], 0), 'b')).toBe('D');
  });

  it('split2-v でもどちらにも出ていなければ null', () => {
    expect(paneBadgeFor(S('split2-v', ['a', 'b'], 0), 'c')).toBeNull();
  });

  // 契約 §28.3: バッジは「どのペインに出ているか」であって「どちらがアクティブか」ではない。
  // activePane を読む実装（例: paneAssignment[s.activePane] を先に見る）に退化させると
  // activePane: 1 側だけが赤くなる（軸 B）
  it('activePane に依存しない（1 始まりでも同じバッジ）', () => {
    expect(paneBadgeFor(S('split2', ['a', 'b'], 1), 'a')).toBe('L');
    expect(paneBadgeFor(S('split2', ['a', 'b'], 1), 'b')).toBe('R');
    expect(paneBadgeFor(S('split2-v', ['a', 'b'], 1), 'a')).toBe('U');
    expect(paneBadgeFor(S('split2-v', ['a', 'b'], 1), 'b')).toBe('D');
  });
});

// 契約 §28.2: 6 箇所の比較の書き漏らしを、向きを変えても挙動が同じであることで検出する。
// 個別タスクの assert が「その関数が退化していないか」を見るのに対し、この describe は
// 「6 箇所が同じ規則に従っているか」を一度に突き合わせる唯一の場所である。
// 7 箇所目が増えた日にここへ 1 本足すこと。重複に見えても削らないこと
describe('split2 と split2-v は向き以外の挙動が同一（契約 §28.2）', () => {
  it('visiblePanes は両方で [0, 1]', () => {
    expect(visiblePanes(S('split2', ['a', 'b'], 0))).toEqual([0, 1]);
    expect(visiblePanes(S('split2-v', ['a', 'b'], 0))).toEqual([0, 1]);
  });

  it('assignPaneReducer は両方で指定ペインに書く', () => {
    expect(assignPaneReducer(S('split2', ['a', null], 0), 1, 'b').paneAssignment).toEqual([
      'a',
      'b',
    ]);
    expect(assignPaneReducer(S('split2-v', ['a', null], 0), 1, 'b').paneAssignment).toEqual([
      'a',
      'b',
    ]);
  });

  it('setActivePaneReducer は両方で activePane を移す', () => {
    expect(setActivePaneReducer(S('split2', ['a', 'b'], 0), 1).activePane).toBe(1);
    expect(setActivePaneReducer(S('split2-v', ['a', 'b'], 0), 1).activePane).toBe(1);
  });

  it('cycleSessionReducer は両方でもう一方のペインのセッションを飛ばす', () => {
    const order = ['a', 'b', 'c'];
    expect(cycleSessionReducer(S('split2', ['a', 'b'], 0), order, 1).paneAssignment[0]).toBe('c');
    expect(cycleSessionReducer(S('split2-v', ['a', 'b'], 0), order, 1).paneAssignment[0]).toBe('c');
  });

  it('routeFocusReducer は両方で割当を動かさず activePane だけ移す', () => {
    for (const layout of ['split2', 'split2-v'] as const) {
      const next = routeFocusReducer(S(layout, ['a', 'b'], 0), 'b');
      expect(next.paneAssignment).toEqual(['a', 'b']);
      expect(next.activePane).toBe(1);
    }
  });

  it('paneBadgeFor は両方で 2 ペインぶんのバッジを返す（向きだけが違う）', () => {
    expect([
      paneBadgeFor(S('split2', ['a', 'b'], 0), 'a'),
      paneBadgeFor(S('split2', ['a', 'b'], 0), 'b'),
    ]).toEqual(['L', 'R']);
    expect([
      paneBadgeFor(S('split2-v', ['a', 'b'], 0), 'a'),
      paneBadgeFor(S('split2-v', ['a', 'b'], 0), 'b'),
    ]).toEqual(['U', 'D']);
  });
});
