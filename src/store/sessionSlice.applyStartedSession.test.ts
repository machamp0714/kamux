import { beforeEach, describe, expect, it } from 'vitest';

import { useAppStore } from './index';
import type { Session } from '../types/model';

// ゲート修正 D-1（brief: .superpowers/sdd/M3-4-ops-ux/gate-fix-brief.md）。
// start_session / resume_session の戻り値が sessions にしか反映されず sessionOrder /
// 盤面の列に反映されない不具合の修正。契約 §144.8 の「局所挿入は buildSessionOrder の
// フィルタを全部迂回する」（正典 §155.4）を自前で当てる。
const s = (over: Partial<Session> & { id: string }): Session => ({
  project_id: 'p1',
  title: over.id,
  description: '',
  kanban_status: 'backlog',
  sort_order: 1,
  mode: 'worktree',
  branch: null,
  worktree_path: null,
  cli_kind: 'claude',
  cli_command: null,
  claude_session_id: null,
  last_runtime_state: 'idle',
  last_runtime_error: null,
  first_started_at: 1,
  heuristics_enabled: true,
  silence_timeout_secs: 30,
  is_scratch: false,
  archived_at: null,
  created_at: 0,
  updated_at: 0,
  ...over,
});

describe('applyStartedSession（ゲート修正 D-1）', () => {
  beforeEach(() => {
    useAppStore.setState({
      activeProjectId: 'p1',
      sessions: {},
      sessionOrder: { backlog: [], in_progress: [], review: [], done: [] },
    });
  });

  // P1
  it('渡された Session を丸ごと sessions[id] へ入れる（branch / worktree_path が入る）', () => {
    const started = s({
      id: 'a',
      kanban_status: 'in_progress',
      branch: 'feat/x',
      worktree_path: '/tmp/x',
    });

    useAppStore.getState().applyStartedSession(started);

    expect(useAppStore.getState().sessions.a).toEqual(started);
  });

  // P3 / P5
  it('backlog に居たカードが in_progress の正しい位置（compareSessionOrder 順）へ移る', () => {
    useAppStore.setState({
      sessions: {
        x: s({ id: 'x', kanban_status: 'in_progress', sort_order: 3 }),
        y: s({ id: 'y', kanban_status: 'in_progress', sort_order: 1 }),
      },
      sessionOrder: { backlog: ['a'], in_progress: ['x', 'y'], review: [], done: [] },
    });
    // sort_order 昇順なら ['y', 'x'] のはずが、意図的にずらしてある
    // （moveCard の楽観更新の in-flight 中に実在する状態。restoreSession の
    // archive.test.ts と同型のケース）。土台の並びは変えないので insertAt は
    // 「x(3), y(1) のうち a(2) より前に来るべき要素の個数」= y の 1 個。
    const started = s({ id: 'a', kanban_status: 'in_progress', sort_order: 2 });

    useAppStore.getState().applyStartedSession(started);

    expect(useAppStore.getState().sessionOrder.in_progress).toEqual(['x', 'a', 'y']);
    expect(useAppStore.getState().sessionOrder.backlog).toEqual([]);
  });

  // P5: compareSessionOrder の引数順を保つ（取り違え検出）。
  // 2 対 1 の非対称な分割にすることで、引数を入れ替えた誤実装
  // （「target より前に来る要素数」ではなく「後に来る要素数」を数える）が
  // 別の挿入位置を返すようにしてある。3 要素中 2 個が target より小さい
  // sort_order を持つため、正しい insertAt=2 に対し、引数を入れ替えると
  // insertAt=1 になる（2 要素だけの対称な入力では両者が偶然一致してしまう）。
  it('挿入位置の探索は compareSessionOrder(既存, 新規) の引数順を保つ', () => {
    useAppStore.setState({
      sessions: {
        p: s({ id: 'p', kanban_status: 'in_progress', sort_order: 1 }),
        q: s({ id: 'q', kanban_status: 'in_progress', sort_order: 2 }),
        r: s({ id: 'r', kanban_status: 'in_progress', sort_order: 3 }),
      },
      sessionOrder: { backlog: [], in_progress: ['p', 'q', 'r'], review: [], done: [] },
    });
    const started = s({ id: 'new', kanban_status: 'in_progress', sort_order: 2.5 });

    useAppStore.getState().applyStartedSession(started);

    expect(useAppStore.getState().sessionOrder.in_progress).toEqual(['p', 'q', 'new', 'r']);
  });

  // gate-fix-round1 R-2（契約 §50.3.2: 並び順は (sort_order, id) の全順序）。
  // archive.test.ts:258 の restoreSession 版と同じ形。sort_order が同値の 'a'/'z' の
  // 間へ、id が両者の間に挟まる 'm' を insertAt で挿む。sort_order だけの比較
  // （タイブレーク無し）だと diff が常に 0 になり insertAt=0（['m','a','z']）になる。
  // 期待値はリテラルで固定する（production の定数や compareSessionOrder からは
  // 再導出しない）。
  it('挿入位置の探索は sort_order 同値のとき id でタイブレークする', () => {
    useAppStore.setState({
      sessions: {
        a: s({ id: 'a', kanban_status: 'in_progress', sort_order: 2 }),
        z: s({ id: 'z', kanban_status: 'in_progress', sort_order: 2 }),
      },
      sessionOrder: { backlog: [], in_progress: ['a', 'z'], review: [], done: [] },
    });
    const started = s({ id: 'm', kanban_status: 'in_progress', sort_order: 2 });

    useAppStore.getState().applyStartedSession(started);

    expect(useAppStore.getState().sessionOrder.in_progress).toEqual(['a', 'm', 'z']);
  });

  // gate-fix-round1 R-3（P4「全 4 列から除去」）。同じ id 'a' が対象列
  // （in_progress）と非対象列の両方に居る初期状態から、
  // applyStartedSession(in_progress の a) を呼ぶ。他列除去ループが非対象列を
  // 回り切ること（S-3 は break で 1 列しか回らなくなる）、かつ
  // targetColumnUnchanged の門（!removedFromOtherColumn）が他列の除去結果を
  // sessionOrder へ実際に反映させること（S-6 は門を落として早期 return し、
  // 非対象列の重複が残る）を toEqual で一度に見る。
  // どの列がどうなるかは下の toEqual がリテラルで固定している。
  // fixture の 'a' は 3 フィルタ（archived_at !== null / is_scratch === true /
  // project_id !== activeProjectId）のどれにも掛からない（s() のデフォルト値と
  // beforeEach の activeProjectId: 'p1' により満たされる）。
  it('同じ id が対象列と別の複数列の両方に居るとき、他列すべてから除去する（P4）', () => {
    useAppStore.setState({
      sessions: { a: s({ id: 'a', kanban_status: 'in_progress' }) },
      sessionOrder: { backlog: ['a'], in_progress: ['a'], review: ['a'], done: ['a'] },
    });
    const started = s({ id: 'a', kanban_status: 'in_progress' });

    useAppStore.getState().applyStartedSession(started);

    expect(useAppStore.getState().sessionOrder).toEqual({
      backlog: [],
      in_progress: ['a'],
      review: [],
      done: [],
    });
  });

  // P2: is_scratch
  it('is_scratch: true の Session を渡しても sessionOrder が変わらない（sessions には入る）', () => {
    const started = s({ id: 'a', kanban_status: 'in_progress', is_scratch: true });

    useAppStore.getState().applyStartedSession(started);

    expect(useAppStore.getState().sessions.a).toEqual(started);
    expect(useAppStore.getState().sessionOrder).toEqual({
      backlog: [],
      in_progress: [],
      review: [],
      done: [],
    });
  });

  // P2: archived_at
  it('archived_at !== null の Session を渡しても sessionOrder が変わらない', () => {
    const started = s({ id: 'a', kanban_status: 'in_progress', archived_at: 1754006400000 });

    useAppStore.getState().applyStartedSession(started);

    expect(useAppStore.getState().sessionOrder).toEqual({
      backlog: [],
      in_progress: [],
      review: [],
      done: [],
    });
  });

  // P2: project_id
  it('project_id !== activeProjectId の Session を渡しても sessionOrder が変わらない', () => {
    const started = s({ id: 'a', project_id: 'other', kanban_status: 'in_progress' });

    useAppStore.getState().applyStartedSession(started);

    expect(useAppStore.getState().sessions.a).toEqual(started);
    expect(useAppStore.getState().sessionOrder).toEqual({
      backlog: [],
      in_progress: [],
      review: [],
      done: [],
    });
  });

  // P4: 冪等性
  it('2 回続けて呼んでも sessionOrder が 1 回目と要素単位で等しい（重複しない）', () => {
    const started = s({ id: 'a', kanban_status: 'in_progress' });

    useAppStore.getState().applyStartedSession(started);
    const firstOrder = useAppStore.getState().sessionOrder.in_progress;

    useAppStore.getState().applyStartedSession(started);

    expect(useAppStore.getState().sessionOrder.in_progress).toEqual(firstOrder);
    expect(useAppStore.getState().sessionOrder.in_progress).toEqual(['a']);
  });

  // P6
  it('列が変わらない Session（既に in_progress）を渡したとき sessionOrder の参照が変わらない', () => {
    useAppStore.setState({
      sessions: { a: s({ id: 'a', kanban_status: 'in_progress' }) },
      sessionOrder: { backlog: [], in_progress: ['a'], review: [], done: [] },
    });
    const beforeOrder = useAppStore.getState().sessionOrder;
    const started = s({ id: 'a', kanban_status: 'in_progress' });

    useAppStore.getState().applyStartedSession(started);

    expect(useAppStore.getState().sessionOrder).toBe(beforeOrder);
    // sessions 側は更新されている（P1 は独立して満たす）
    expect(useAppStore.getState().sessions.a).toEqual(started);
  });
});
