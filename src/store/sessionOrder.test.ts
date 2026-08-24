import { describe, expect, it } from 'vitest';
import type { Session } from '../types/model';
import { buildSessionOrder, KANBAN_STATUSES } from './sessionOrder';

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
  archived_at: null,
  created_at: 0,
  updated_at: 0,
  ...over,
});

describe('buildSessionOrder', () => {
  it('4 列すべてのキーを常に持つ', () => {
    const order = buildSessionOrder([], 'p1');
    expect(Object.keys(order).sort()).toEqual([...KANBAN_STATUSES].sort());
    expect(order.done).toEqual([]);
  });

  it('sort_order の昇順に並べる', () => {
    const order = buildSessionOrder(
      [s({ id: 'c', sort_order: 3 }), s({ id: 'a', sort_order: 1 }), s({ id: 'b', sort_order: 2 })],
      'p1',
    );
    expect(order.backlog).toEqual(['a', 'b', 'c']);
  });

  it('他プロジェクトのセッションを除外する', () => {
    const order = buildSessionOrder(
      [s({ id: 'mine', project_id: 'p1' }), s({ id: 'other', project_id: 'p2' })],
      'p1',
    );
    expect(order.backlog).toEqual(['mine']);
  });

  it('アーカイブ済みセッションをボードから除外する', () => {
    const order = buildSessionOrder(
      [
        s({ id: 'live', kanban_status: 'done', sort_order: 1 }),
        s({ id: 'archived', kanban_status: 'done', sort_order: 2, archived_at: 1754006400000 }),
      ],
      'p1',
    );
    expect(order.done).toEqual(['live']);
  });

  it('列ごとに振り分ける', () => {
    const order = buildSessionOrder(
      [
        s({ id: 'a', kanban_status: 'backlog' }),
        s({ id: 'b', kanban_status: 'in_progress' }),
        s({ id: 'c', kanban_status: 'review' }),
        s({ id: 'd', kanban_status: 'done' }),
      ],
      'p1',
    );
    expect(order).toEqual({ backlog: ['a'], in_progress: ['b'], review: ['c'], done: ['d'] });
  });
});
