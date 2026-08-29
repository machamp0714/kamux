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
