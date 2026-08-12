import { beforeEach, describe, expect, it, vi } from 'vitest';

const listProjects = vi.fn();
const createProject = vi.fn();
const listSessions = vi.fn();
vi.mock('../ipc/commands', () => ({
  listProjects: (...args: unknown[]) => listProjects(...args),
  createProject: (...args: unknown[]) => createProject(...args),
  listSessions: (...args: unknown[]) => listSessions(...args),
  createSession: vi.fn(),
  updateSession: vi.fn(),
}));

import type { Project, Session } from '../types/model';
import { useAppStore } from './index';
import { ACTIVE_PROJECT_STORAGE_KEY } from './projectSlice';
import { emptySessionOrder } from './sessionSlice';

const project = (id: string): Project => ({
  id,
  name: id,
  repo_path: `/repo/${id}`,
  default_cli: 'claude',
  created_at: 1,
  updated_at: 1,
});

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
  heuristics_enabled: true,
  silence_timeout_secs: 30,
  archived_at: null,
  created_at: 1,
  updated_at: 1,
  ...over,
});

beforeEach(() => {
  listProjects.mockReset();
  createProject.mockReset();
  listSessions.mockReset().mockResolvedValue([]);
  localStorage.clear();
  useAppStore.setState({
    projects: [],
    activeProjectId: null,
    sessions: {},
    sessionOrder: emptySessionOrder(),
  });
});

describe('ACTIVE_PROJECT_STORAGE_KEY', () => {
  it('契約で確定した文字列そのものである', () => {
    expect(ACTIVE_PROJECT_STORAGE_KEY).toBe('kamux.activeProjectId');
  });
});

describe('loadProjects', () => {
  it('取得したプロジェクトをストアに入れる', async () => {
    listProjects.mockResolvedValue([project('p1'), project('p2')]);
    await useAppStore.getState().loadProjects();
    expect(useAppStore.getState().projects.map((p) => p.id)).toEqual(['p1', 'p2']);
  });

  it('activeProjectId を勝手に決めない（localStorage も書かない）', async () => {
    listProjects.mockResolvedValue([project('p1')]);
    await useAppStore.getState().loadProjects();
    expect(useAppStore.getState().activeProjectId).toBeNull();
    expect(localStorage.getItem(ACTIVE_PROJECT_STORAGE_KEY)).toBeNull();
  });
});

describe('setActiveProject', () => {
  it('アクティブ ID を立て、localStorage に保存し、セッションを読み直す', async () => {
    await useAppStore.getState().setActiveProject('p2');

    expect(useAppStore.getState().activeProjectId).toBe('p2');
    expect(localStorage.getItem(ACTIVE_PROJECT_STORAGE_KEY)).toBe('p2');
    expect(listSessions).toHaveBeenCalledWith('p2', false);
  });

  it('loadSessions の完了を待ってから返る（await 落ちを検出する）', async () => {
    // listSessions の解決タイミングを手動で制御する。`sessions` の状態を見て判定する形
    // （前回の実装）だと、マイクロタスクの実行順がたまたま噛み合うことで
    // `await get().loadSessions(id)` を `void get().loadSessions(id)` に書き換えても
    // 緑のままになりうる（loadSessions 内部の継続が setActiveProject の返り値の
    // 解決より先にキューへ積まれるため）。ここでは setActiveProject 自身が返す
    // Promise が「listSessions が解決するまで settle しない」ことを直接検証する。
    let resolveListSessions!: (sessions: Session[]) => void;
    listSessions.mockReturnValue(
      new Promise<Session[]>((resolve) => {
        resolveListSessions = resolve;
      }),
    );

    let settled = false;
    const pending = useAppStore
      .getState()
      .setActiveProject('p2')
      .then(() => {
        settled = true;
      });

    // listSessions が未解決の間、マイクロタスクを何ターン消化しても
    // setActiveProject の Promise は解決しないはず（await 落ちなら即 settle する）。
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    expect(settled).toBe(false);

    resolveListSessions([session({ id: 's-in-p2', project_id: 'p2' })]);
    await pending;

    expect(settled).toBe(true);
    expect(Object.keys(useAppStore.getState().sessions)).toEqual(['s-in-p2']);
  });
});

describe('addProject', () => {
  it('作成したプロジェクトを一覧の末尾に足して返す', async () => {
    listProjects.mockResolvedValue([project('p1')]);
    await useAppStore.getState().loadProjects();

    createProject.mockResolvedValue(project('p2'));
    const created = await useAppStore.getState().addProject('p2', '/repo/p2', 'claude');

    expect(created.id).toBe('p2');
    expect(createProject).toHaveBeenCalledWith('p2', '/repo/p2', 'claude');
    expect(useAppStore.getState().projects.map((p) => p.id)).toEqual(['p1', 'p2']);
  });

  it('作成してもアクティブは切り替えない（呼び出し側の判断に任せる）', async () => {
    createProject.mockResolvedValue(project('p2'));
    await useAppStore.getState().addProject('p2', '/repo/p2', 'claude');
    expect(useAppStore.getState().activeProjectId).toBeNull();
    expect(localStorage.getItem(ACTIVE_PROJECT_STORAGE_KEY)).toBeNull();
  });
});
