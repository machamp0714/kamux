import { describe, expect, it } from 'vitest';
import type { Session } from '../types/model';
import {
  buildSessionOrder,
  emptySessionOrder,
  indexSessions,
  moveCardInOrder,
} from './kanbanOrder';

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
    heuristics_enabled: true,
    silence_timeout_secs: 30,
    is_scratch: false,
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

  it('is_scratch のセッションを除外する（契約 §29.4、主たる境界）', () => {
    const order = buildSessionOrder(
      toMap([
        makeSession({ id: 'scratch1', kanban_status: 'backlog', sort_order: 1, is_scratch: true }),
        makeSession({ id: 'normal1', kanban_status: 'backlog', sort_order: 2, is_scratch: false }),
      ]),
    );
    expect(order.backlog).toEqual(['normal1']);
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

function order(partial: Partial<Record<string, string[]>>) {
  return {
    backlog: partial.backlog ?? [],
    in_progress: partial.in_progress ?? [],
    review: partial.review ?? [],
    done: partial.done ?? [],
  };
}

describe('moveCardInOrder', () => {
  it('同じ列で下方向へ動かす（arrayMove と同じ結果になる）', () => {
    const next = moveCardInOrder(order({ backlog: ['a', 'b', 'c'] }), 'a', 'backlog', 2);
    expect(next.backlog).toEqual(['b', 'c', 'a']);
  });

  it('同じ列で上方向へ動かす', () => {
    const next = moveCardInOrder(order({ backlog: ['a', 'b', 'c'] }), 'c', 'backlog', 0);
    expect(next.backlog).toEqual(['c', 'a', 'b']);
  });

  it('同じ列で 1 つだけ下へ動かす（off-by-one の回帰テスト）', () => {
    const next = moveCardInOrder(order({ backlog: ['a', 'b', 'c'] }), 'a', 'backlog', 1);
    expect(next.backlog).toEqual(['b', 'a', 'c']);
  });

  it('列をまたいで動かす', () => {
    const next = moveCardInOrder(
      order({ backlog: ['a', 'b'], in_progress: ['x', 'y'] }),
      'a',
      'in_progress',
      1,
    );
    expect(next.backlog).toEqual(['b']);
    expect(next.in_progress).toEqual(['x', 'a', 'y']);
  });

  it('末尾より大きい index は末尾にクランプする', () => {
    const next = moveCardInOrder(order({ backlog: ['a', 'b', 'c'] }), 'a', 'backlog', 99);
    expect(next.backlog).toEqual(['b', 'c', 'a']);
  });

  it('空の列へ動かせる', () => {
    const next = moveCardInOrder(order({ backlog: ['a'] }), 'a', 'done', 0);
    expect(next.backlog).toEqual([]);
    expect(next.done).toEqual(['a']);
  });

  it('全列から id を除去する（M1-1 の防御的挙動を維持）', () => {
    // 何らかの理由で 2 列に同じ id が入っていても、移動後に重複が残らないこと
    const next = moveCardInOrder(
      order({ backlog: ['a', 'b'], review: ['a'], done: ['c'] }),
      'a',
      'done',
      0,
    );
    expect(next.backlog).toEqual(['b']);
    expect(next.review).toEqual([]);
    expect(next.done).toEqual(['a', 'c']);
  });

  it('入力の SessionOrder を破壊しない', () => {
    const before = order({ backlog: ['a', 'b'] });
    moveCardInOrder(before, 'a', 'review', 0);
    expect(before.backlog).toEqual(['a', 'b']);
    expect(before.review).toEqual([]);
  });
});
