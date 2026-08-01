import { describe, expect, it } from 'vitest';
import type { Session } from '../types/model';
import { buildSessionOrder, emptySessionOrder, indexSessions } from './kanbanOrder';

export function makeSession(overrides: Partial<Session> & { id: string }): Session {
  return {
    project_id: 'p1',
    title: overrides.id,
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
    archived_at: null,
    created_at: 0,
    updated_at: 0,
    ...overrides,
  };
}

function toMap(list: Session[]): Record<string, Session> {
  return Object.fromEntries(list.map((s) => [s.id, s]));
}

describe('buildSessionOrder', () => {
  it('kanban_status ごとに sort_order 昇順で並べる', () => {
    const order = buildSessionOrder(
      toMap([
        makeSession({ id: 'b2', kanban_status: 'backlog', sort_order: 2 }),
        makeSession({ id: 'b1', kanban_status: 'backlog', sort_order: 1 }),
        makeSession({ id: 'r1', kanban_status: 'review', sort_order: 5 }),
      ]),
    );
    expect(order.backlog).toEqual(['b1', 'b2']);
    expect(order.review).toEqual(['r1']);
    expect(order.in_progress).toEqual([]);
  });

  it('archived_at が非 null のセッションを除外する', () => {
    const order = buildSessionOrder(
      toMap([
        makeSession({ id: 'a', kanban_status: 'done', sort_order: 1, archived_at: 1754006400000 }),
        makeSession({ id: 'b', kanban_status: 'done', sort_order: 2 }),
      ]),
    );
    expect(order.done).toEqual(['b']);
  });

  it('sort_order が同値のときは id の辞書順で安定させる', () => {
    const order = buildSessionOrder(
      toMap([
        makeSession({ id: 'zzz', kanban_status: 'backlog', sort_order: 1 }),
        makeSession({ id: 'aaa', kanban_status: 'backlog', sort_order: 1 }),
      ]),
    );
    expect(order.backlog).toEqual(['aaa', 'zzz']);
  });

  it('空の入力で 4 列すべて空を返す', () => {
    expect(buildSessionOrder({})).toEqual(emptySessionOrder());
  });

  it('入力のセッションオブジェクトを破壊しない', () => {
    const sessions = toMap([makeSession({ id: 'a', sort_order: 3 })]);
    buildSessionOrder(sessions);
    expect(sessions.a.sort_order).toBe(3);
  });
});

describe('indexSessions', () => {
  it('id をキーにしたマップと列の並びを同時に返す', () => {
    const result = indexSessions([
      makeSession({ id: 'b', kanban_status: 'backlog', sort_order: 2 }),
      makeSession({ id: 'a', kanban_status: 'backlog', sort_order: 1 }),
    ]);
    expect(Object.keys(result.sessions).sort()).toEqual(['a', 'b']);
    expect(result.sessionOrder.backlog).toEqual(['a', 'b']);
  });

  it('buildSessionOrder と同じ並びを返す', () => {
    const list = [
      makeSession({ id: 'y', kanban_status: 'review', sort_order: 1 }),
      makeSession({ id: 'x', kanban_status: 'review', sort_order: 1 }),
    ];
    expect(indexSessions(list).sessionOrder).toEqual(buildSessionOrder(toMap(list)));
  });
});
