import { beforeEach, describe, expect, it, vi } from 'vitest';

const listSessions = vi.fn();
const createSession = vi.fn();
vi.mock('../ipc/commands', () => ({
  listSessions: (...args: unknown[]) => listSessions(...args),
  createSession: (...args: unknown[]) => createSession(...args),
  updateSession: vi.fn(),
  listProjects: vi.fn(),
  createProject: vi.fn(),
}));

import type { Session } from '../types/model';
import { useAppStore } from './index';
import { emptySessionOrder, indexSessions } from './sessionSlice';

const session = (over: Partial<Session>): Session => ({
  id: 's1',
  project_id: 'p1',
  title: 't',
  description: '',
  kanban_status: 'backlog',
  sort_order: 1,
  mode: 'in_place',
  branch: null,
  worktree_path: null,
  cli_kind: 'shell',
  cli_command: null,
  claude_session_id: null,
  last_runtime_state: 'idle',
  last_runtime_error: null,
  first_started_at: null,
  archived_at: null,
  created_at: 1,
  updated_at: 1,
  ...over,
});

beforeEach(() => {
  listSessions.mockReset();
  createSession.mockReset();
  useAppStore.setState({ sessions: {}, sessionOrder: emptySessionOrder() });
});

describe('emptySessionOrder', () => {
  it('4 列すべてを空配列で持つ', () => {
    expect(emptySessionOrder()).toEqual({ backlog: [], in_progress: [], review: [], done: [] });
  });

  it('呼ぶたびに新しいオブジェクトを返す（列配列を共有しない）', () => {
    const a = emptySessionOrder();
    a.backlog.push('x');
    expect(emptySessionOrder().backlog).toEqual([]);
  });
});

describe('indexSessions', () => {
  it('id 索引と列ごとの sort_order 昇順を作る', () => {
    const { sessions, sessionOrder } = indexSessions([
      session({ id: 'b', kanban_status: 'backlog', sort_order: 2 }),
      session({ id: 'a', kanban_status: 'backlog', sort_order: 1 }),
      session({ id: 'c', kanban_status: 'review', sort_order: 5 }),
    ]);

    expect(Object.keys(sessions).sort()).toEqual(['a', 'b', 'c']);
    expect(sessionOrder.backlog).toEqual(['a', 'b']);
    expect(sessionOrder.review).toEqual(['c']);
    expect(sessionOrder.in_progress).toEqual([]);
    expect(sessionOrder.done).toEqual([]);
  });

  it('空配列でも 4 列を返す', () => {
    expect(indexSessions([]).sessionOrder).toEqual(emptySessionOrder());
  });

  it('sort_order 同値なら入力配列の順を保つ（フロントで id 再ソートしない）', () => {
    // Rust 側の list_sessions は ORDER BY kanban_status, sort_order, id で返すため、
    // バックエンドが渡した配列はすでに id で決着済み。フロント側が改めて id で
    // 再ソートすると、たまたま一致するだけの実装になり退行を検出できなくなる。
    // ここではあえて id 降順（'z' → 'a'）で渡し、indexSessions が入力順をそのまま
    // 保持する（= 黙って id 昇順に並べ替えない）ことを固定する。
    const { sessionOrder } = indexSessions([
      session({ id: 'z', kanban_status: 'backlog', sort_order: 1 }),
      session({ id: 'a', kanban_status: 'backlog', sort_order: 1 }),
    ]);

    expect(sessionOrder.backlog).toEqual(['z', 'a']);
  });
});

describe('loadSessions', () => {
  it('アーカイブ済みを除いて取得し、ストアに展開する', async () => {
    listSessions.mockResolvedValue([
      session({ id: 'a', sort_order: 2 }),
      session({ id: 'b', sort_order: 1 }),
    ]);

    await useAppStore.getState().loadSessions('p1');

    expect(listSessions).toHaveBeenCalledWith('p1', false);
    expect(useAppStore.getState().sessionOrder.backlog).toEqual(['b', 'a']);
    expect(useAppStore.getState().sessions.a.sort_order).toBe(2);
  });

  it('プロジェクトを切り替えると前のセッションを完全に置き換える', async () => {
    listSessions.mockResolvedValue([session({ id: 'old' })]);
    await useAppStore.getState().loadSessions('p1');

    listSessions.mockResolvedValue([session({ id: 'new', project_id: 'p2' })]);
    await useAppStore.getState().loadSessions('p2');

    expect(listSessions).toHaveBeenNthCalledWith(2, 'p2', false);
    expect(Object.keys(useAppStore.getState().sessions)).toEqual(['new']);
    expect(useAppStore.getState().sessionOrder.backlog).toEqual(['new']);
  });
});

describe('addSession', () => {
  it('作成したセッションを backlog の末尾に足す', async () => {
    listSessions.mockResolvedValue([session({ id: 'a', sort_order: 1 })]);
    await useAppStore.getState().loadSessions('p1');

    // サーバ応答の title を args とあえて異なる値にする。
    // `sessions[created.id]` が「サーバ応答（createSession の戻り値）をそのまま格納」
    // しているのか「args から Session を再構成」しているのかを、この差異で区別できる
    // ようにする（args を転記しただけの実装では created と一致しなくなる）。
    const serverSession = session({ id: 'b', sort_order: 2, title: 'new-from-server' });
    createSession.mockResolvedValue(serverSession);
    const args = {
      projectId: 'p1',
      title: 'new',
      description: '',
      mode: 'in_place' as const,
      branch: null,
      cliKind: 'shell' as const,
      cliCommand: null,
    };
    const created = await useAppStore.getState().addSession(args);

    expect(createSession).toHaveBeenCalledWith(args);
    expect(created).toEqual(serverSession);
    expect(useAppStore.getState().sessionOrder.backlog).toEqual(['a', 'b']);
    // sessions（id 索引）にサーバ応答そのものが格納されていることを検証する。
    expect(useAppStore.getState().sessions.b).toEqual(created);
  });

  it('backlog 以外の列で作られたセッションはその列に足す（backlog 決め打ちを検出する）', async () => {
    const serverSession = session({ id: 'r', kanban_status: 'review', sort_order: 1 });
    createSession.mockResolvedValue(serverSession);

    await useAppStore.getState().addSession({
      projectId: 'p1',
      title: 'r',
      description: '',
      mode: 'in_place',
      branch: null,
      cliKind: 'shell',
      cliCommand: null,
    });

    expect(useAppStore.getState().sessionOrder.review).toEqual(['r']);
    expect(useAppStore.getState().sessionOrder.backlog).toEqual([]);
    // sessions（id 索引）側にも review 列のセッションが入っていることを検証する。
    // sessionOrder だけ更新して sessions を更新し忘れる退行を拾う。
    expect(useAppStore.getState().sessions.r).toEqual(serverSession);
  });
});
