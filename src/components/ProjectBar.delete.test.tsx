import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

const listSessions = vi.fn();
const stopSession = vi.fn();
const deleteProject = vi.fn();
vi.mock('../ipc/commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../ipc/commands')>()),
  listSessions: (...a: unknown[]) => listSessions(...a),
  stopSession: (...a: unknown[]) => stopSession(...a),
  deleteProject: (...a: unknown[]) => deleteProject(...a),
}));

import { useAppStore } from '../store';
import { emptySessionOrder } from '../store/sessionSlice';
import type { Project, RuntimeState, Session } from '../types/model';
import { ProjectBar } from './ProjectBar';

afterEach(cleanup);

const project = (id: string, name: string): Project => ({
  id,
  name,
  repo_path: `/repo/${name}`,
  default_cli: 'claude',
  created_at: 1,
  updated_at: 1,
});

const session = (id: string, projectId: string): Session => ({
  id,
  project_id: projectId,
  title: id,
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
});

/**
 * 3 プロジェクト立てる。候補が 1 つしか残らない fixture だと「残りの先頭」と
 * 「残りの末尾」が同じ答えになり、契約 §130.5 のケース 2 が無条件に緑になる。
 */
const PROJECTS = [project('p1', 'kamux'), project('p2', 'beta'), project('p3', 'gamma')];

/**
 * p1 のセッションは 3 件で、実行状態は running / 未登録 / idle の 3 通り。
 * 稼働中に数えてよいのは running の 1 件だけである（契約 §38.3 論点 2 —— 未登録は
 * 「実行状態が未知」であって「稼働中」ではない。DB の last_runtime_state で埋めない）。
 */
const SESSIONS: Record<string, Session> = {
  s1: session('s1', 'p1'),
  s2: session('s2', 'p1'),
  s3: session('s3', 'p1'),
  s4: session('s4', 'p2'),
};
const RUNTIME_STATES: Record<string, RuntimeState> = { s1: 'running', s3: 'idle', s4: 'running' };

beforeEach(() => {
  listSessions.mockReset().mockResolvedValue([]);
  stopSession.mockReset().mockResolvedValue(undefined);
  deleteProject.mockReset().mockResolvedValue(undefined);
  localStorage.clear();
  useAppStore.setState({
    projects: PROJECTS,
    activeProjectId: 'p1',
    sessions: { ...SESSIONS },
    sessionOrder: { ...emptySessionOrder(), backlog: ['s1', 's2', 's3'] },
    runtimeStates: { ...RUNTIME_STATES },
    deleteProjectDialog: null,
    modal: null,
    cleanupDialog: null,
    projectSwitcherOpen: false,
    layout: 'single',
    paneAssignment: ['s1', null],
    activePane: 0,
    focusedSessionId: 's1',
    workspaceByProject: {},
  });
});

const clickDelete = (name: string) =>
  fireEvent.click(screen.getByRole('button', { name: `${name} を削除` }));

const confirmDelete = () => {
  fireEvent.click(screen.getByRole('checkbox'));
  fireEvent.click(screen.getByRole('button', { name: '削除する' }));
};

describe('ProjectBar の削除導線', () => {
  // 契約 §7.1: 破壊操作は確認ダイアログを挟む。押した瞬間に消してはならない。
  it('削除ボタンを押しても消さない —— 確認ダイアログが開くだけ（契約 §7.1）', () => {
    render(<ProjectBar />);

    clickDelete('kamux');

    expect(deleteProject).not.toHaveBeenCalled();
    expect(stopSession).not.toHaveBeenCalled();
    // 「呼ばれない」だけを見ると何も描かれていなくても通る。開いたことを添える。
    expect(screen.getByRole('dialog', { name: 'プロジェクトを削除' })).toBeTruthy();
    expect(useAppStore.getState().projects.map((p) => p.id)).toEqual(['p1', 'p2', 'p3']);
  });

  it('ダイアログは消えるセッション数と稼働中の件数を出す（未登録の実行状態は稼働中に数えない）', () => {
    render(<ProjectBar />);

    clickDelete('kamux');

    // 3 と 1 は取り違えたら別物になる具体値。s4（別プロジェクトの running）は数えない。
    expect(screen.getByText('セッション 3 件が一緒に消えます')).toBeTruthy();
    expect(screen.getByText('うち稼働中 1 件')).toBeTruthy();
  });

  it('キャンセルするとダイアログが閉じ、何も消えない', () => {
    render(<ProjectBar />);

    clickDelete('kamux');
    fireEvent.click(screen.getByRole('button', { name: 'キャンセル' }));

    expect(screen.queryByRole('dialog', { name: 'プロジェクトを削除' })).toBeNull();
    expect(deleteProject).not.toHaveBeenCalled();
    expect(useAppStore.getState().projects.map((p) => p.id)).toEqual(['p1', 'p2', 'p3']);
  });

  // 契約 §130.4 / §147.2: sessions は §3 の ON DELETE CASCADE で消えるので、
  // 行だけが消えて PTY が生き残ると孤児になる。stop_session は冪等なので
  // 稼働中かどうかで分岐せず、対象プロジェクトの全セッションへ無差別に回す。
  it('確定すると対象プロジェクトの全セッションを止めてから delete_project を呼ぶ（契約 §130.4）', async () => {
    render(<ProjectBar />);

    clickDelete('kamux');
    confirmDelete();

    await waitFor(() => expect(deleteProject).toHaveBeenCalledWith('p1'));
    // 稼働中の s1 だけでなく、未登録の s2 と idle の s3 にも回す（無差別）。
    const stopped = stopSession.mock.calls.map((c) => c[0] as string).sort();
    expect(stopped).toEqual(['s1', 's2', 's3']);
    // 別プロジェクトのセッションには回さない（project id と session id は同じ素の string）。
    expect(stopSession).not.toHaveBeenCalledWith('s4');
    expect(stopSession).not.toHaveBeenCalledWith('p1');
    // 順序。stop が delete の後ろだと、CASCADE で行が消えた後に止めることになる。
    const lastStop = Math.max(...stopSession.mock.invocationCallOrder);
    expect(lastStop).toBeLessThan(deleteProject.mock.invocationCallOrder[0]);
  });

  // 契約 §130.5 の 3 ケース。
  it('非アクティブを削除しても activeProjectId は動かない（契約 §130.5）', async () => {
    render(<ProjectBar />);

    clickDelete('beta');
    confirmDelete();

    await waitFor(() => expect(deleteProject).toHaveBeenCalledWith('p2'));
    await waitFor(() => expect(useAppStore.getState().projects).toHaveLength(2));
    expect(useAppStore.getState().projects.map((p) => p.id)).toEqual(['p1', 'p3']);
    expect(useAppStore.getState().activeProjectId).toBe('p1');
    // 切り替えていないので盤面は取り直さない。
    expect(listSessions).not.toHaveBeenCalled();
  });

  it('アクティブを削除すると残りの先頭へ落ちる（末尾ではない。契約 §130.5）', async () => {
    render(<ProjectBar />);

    clickDelete('kamux');
    confirmDelete();

    await waitFor(() => expect(useAppStore.getState().activeProjectId).toBe('p2'));
    expect(useAppStore.getState().activeProjectId).not.toBe('p3');
    expect(useAppStore.getState().projects.map((p) => p.id)).toEqual(['p2', 'p3']);
    // setActiveProject を通した観測点。盤面・ワークスペース・§85.1 の維持はそこが持つ。
    expect(listSessions).toHaveBeenCalledWith('p2', true);
    expect(localStorage.getItem('kamux.activeProjectId')).toBe('p2');
  });

  it('最後の 1 つを削除すると未選択になり、盤面が空になる（契約 §130.5 / §85.1）', async () => {
    useAppStore.setState({ projects: [project('p1', 'kamux')] });
    render(<ProjectBar />);

    clickDelete('kamux');
    confirmDelete();

    await waitFor(() => expect(useAppStore.getState().activeProjectId).toBeNull());
    const st = useAppStore.getState();
    expect(st.projects).toEqual([]);
    expect(st.sessionOrder).toEqual(emptySessionOrder());
    expect(st.paneAssignment).toEqual([null, null]);
    // 契約 §85.1: focusedSessionId === paneAssignment[activePane]
    expect(st.focusedSessionId).toBe(st.paneAssignment[st.activePane]);
    expect(st.focusedSessionId).toBeNull();
  });

  it('確定するとダイアログが閉じる', async () => {
    render(<ProjectBar />);

    clickDelete('kamux');
    confirmDelete();

    await waitFor(() =>
      expect(screen.queryByRole('dialog', { name: 'プロジェクトを削除' })).toBeNull(),
    );
    // 「消えた」だけを見るとバー自体が描かれていなくても通る。バーが在ることを添える。
    expect(screen.getByRole('button', { name: 'beta を削除' })).toBeTruthy();
  });
});
