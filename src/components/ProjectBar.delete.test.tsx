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
 * `st.sessions` に載っているのはアクティブプロジェクト（p1）の分だけである。
 * `loadSessions` は「置換ではなくマージ」で、呼ばれるのはプロジェクトを開いたときだけ
 * なので、一度もアクティブにしていない p2 / p3 のセッションはここに 1 件も無い。
 * 🔴 ダイアログの件数と `stop_session` の対象を `st.sessions` から引くと、
 * 未訪問プロジェクトでは常に 0 件になる（契約 §130.4 が到達可能な経路で成立しなくなる）。
 */
const MERGED_SESSIONS: Record<string, Session> = {
  a1: session('a1', 'p1'),
  a2: session('a2', 'p1'),
};

/**
 * `list_sessions(projectId, true)` が返すもの。プロジェクトごとに別の配列を返す ——
 * 全プロジェクトで同じ配列を返すと「対象プロジェクトの分だけ数えている」が測れない。
 *
 * p2 の 4 件は 4 通りの実行状態を 1 枠ずつ持つ。稼働中に数えてよいのは s1 と s5 の
 * 2 件だけである:
 *   s1 = runtimeStates に `running`（live 判定の陽性）
 *   s2 = runtimeStates に**未登録**で DB の last_runtime_state だけが `running`
 *        —— 契約 §38.3 論点 2 が名指しで禁じた `?? last_runtime_state` を打つと 3 件に動く
 *   s3 = runtimeStates に `idle`（idle を live に数えない）
 *   s5 = runtimeStates に `waiting_input`（契約 §38.3 の live 2 値の片側）
 */
const REMOTE_SESSIONS: Record<string, Session[]> = {
  p1: [MERGED_SESSIONS.a1, MERGED_SESSIONS.a2],
  p2: [
    session('s1', 'p2'),
    { ...session('s2', 'p2'), last_runtime_state: 'running' },
    session('s3', 'p2'),
    session('s5', 'p2'),
  ],
  p3: [],
};

const RUNTIME_STATES: Record<string, RuntimeState> = {
  a1: 'running',
  s1: 'running',
  s3: 'idle',
  s5: 'waiting_input',
};

beforeEach(() => {
  listSessions
    .mockReset()
    .mockImplementation((projectId: string) => Promise.resolve(REMOTE_SESSIONS[projectId] ?? []));
  stopSession.mockReset().mockResolvedValue(undefined);
  deleteProject.mockReset().mockResolvedValue(undefined);
  localStorage.clear();
  useAppStore.setState({
    projects: PROJECTS,
    activeProjectId: 'p1',
    sessions: { ...MERGED_SESSIONS },
    sessionOrder: { ...emptySessionOrder(), backlog: ['a1', 'a2'] },
    runtimeStates: { ...RUNTIME_STATES },
    deleteProjectDialog: null,
    modal: null,
    cleanupDialog: null,
    projectSwitcherOpen: false,
    layout: 'single',
    paneAssignment: ['a1', null],
    activePane: 0,
    focusedSessionId: 'a1',
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
  it('削除ボタンを押しても消さない —— 確認ダイアログが開くだけ（契約 §7.1）', async () => {
    render(<ProjectBar />);

    clickDelete('kamux');

    // 「呼ばれない」だけを見ると何も描かれていなくても通る。開いたことを添える。
    expect(await screen.findByText('セッション 2 件が一緒に消えます')).toBeTruthy();
    expect(deleteProject).not.toHaveBeenCalled();
    expect(stopSession).not.toHaveBeenCalled();
    expect(screen.getByRole('dialog', { name: 'プロジェクトを削除' })).toBeTruthy();
    expect(useAppStore.getState().projects.map((p) => p.id)).toEqual(['p1', 'p2', 'p3']);
  });

  // 🔴 対象は一度もアクティブにしていない p2 である。`st.sessions` には p2 の行が
  // 1 件も無いので、母数を `st.sessions` から引く形なら 0 件になる（契約 §130.4）。
  it('未訪問プロジェクトでも消えるセッション数と稼働中の件数を出す（契約 §130.4）', async () => {
    render(<ProjectBar />);

    clickDelete('beta');

    // 4 と 2 は取り違えたら別物になる具体値。p1 の a1（running）は数えない。
    expect(await screen.findByText('セッション 4 件が一緒に消えます')).toBeTruthy();
    expect(screen.getByText('うち稼働中 2 件')).toBeTruthy();
    // 契約 §3 の ON DELETE CASCADE はアーカイブ済みの行も消す。母数に入れる。
    expect(listSessions).toHaveBeenCalledWith('p2', true);
  });

  // 知らないときに断定しない（契約 §38.3 論点 2 と同じ規律）。「0 件」と書かない。
  it('件数が取れるまでは件数を主張しない（0 件と書かない）', async () => {
    render(<ProjectBar />);

    clickDelete('beta');

    // list_sessions の応答はまだ適用されていない（microtask を 1 度も回していない）。
    expect(screen.getByRole('dialog', { name: 'プロジェクトを削除' })).toBeTruthy();
    expect(screen.queryByText(/件が一緒に消えます/)).toBeNull();
    expect(screen.queryByText(/うち稼働中/)).toBeNull();
    expect(screen.getByText('セッションを数えています…')).toBeTruthy();

    // 「いつまでも出ない」ではないことを添える。
    expect(await screen.findByText('セッション 4 件が一緒に消えます')).toBeTruthy();
  });

  it('キャンセルするとダイアログが閉じ、何も消えない', async () => {
    render(<ProjectBar />);

    clickDelete('kamux');
    await screen.findByText('セッション 2 件が一緒に消えます');
    fireEvent.click(screen.getByRole('button', { name: 'キャンセル' }));

    expect(screen.queryByRole('dialog', { name: 'プロジェクトを削除' })).toBeNull();
    expect(deleteProject).not.toHaveBeenCalled();
    expect(useAppStore.getState().projects.map((p) => p.id)).toEqual(['p1', 'p2', 'p3']);
  });

  // 契約 §130.4 / §147.2: sessions は §3 の ON DELETE CASCADE で消えるので、
  // 行だけが消えて PTY が生き残ると孤児になる。stop_session は冪等なので
  // 稼働中かどうかで分岐せず、対象プロジェクトの全セッションへ無差別に回す。
  // 🔴 対象は未訪問の p2 —— `st.sessions` から引く形なら 1 件も止まらない。
  it('確定すると list_sessions が返した全セッションを止めてから delete_project を呼ぶ（契約 §130.4）', async () => {
    render(<ProjectBar />);

    clickDelete('beta');
    await screen.findByText('セッション 4 件が一緒に消えます');
    confirmDelete();

    await waitFor(() => expect(deleteProject).toHaveBeenCalledWith('p2'));
    // 稼働中の s1 / s5 だけでなく、未登録の s2 と idle の s3 にも回す（無差別）。
    const stopped = stopSession.mock.calls.map((c) => c[0] as string).sort();
    expect(stopped).toEqual(['s1', 's2', 's3', 's5']);
    // 別プロジェクトのセッションには回さない（project id と session id は同じ素の string）。
    expect(stopSession).not.toHaveBeenCalledWith('a1');
    expect(stopSession).not.toHaveBeenCalledWith('p2');
    // 順序。stop が delete の後ろだと、CASCADE で行が消えた後に止めることになる。
    const lastStop = Math.max(...stopSession.mock.invocationCallOrder);
    expect(lastStop).toBeLessThan(deleteProject.mock.invocationCallOrder[0]);
  });

  // 契約 §130.5 の 3 ケース。
  it('非アクティブを削除しても activeProjectId は動かない（契約 §130.5）', async () => {
    render(<ProjectBar />);

    clickDelete('beta');
    await screen.findByText('セッション 4 件が一緒に消えます');
    confirmDelete();

    await waitFor(() => expect(deleteProject).toHaveBeenCalledWith('p2'));
    await waitFor(() => expect(useAppStore.getState().projects).toHaveLength(2));
    expect(useAppStore.getState().projects.map((p) => p.id)).toEqual(['p1', 'p3']);
    expect(useAppStore.getState().activeProjectId).toBe('p1');
    // 切り替えていないので盤面は取り直さない。件数のための 1 往復だけが立つ。
    expect(listSessions.mock.calls).toEqual([['p2', true]]);
  });

  // 🔴 上のケースは activeProjectId('p1') が remaining の先頭とも一致してしまうため、
  // 「常に remaining[0] へ落とす」退行を区別できない。落とし先と一致しない配置で 1 本測る。
  it('非アクティブを削除したとき、残りの先頭がアクティブでなくても動かない（契約 §130.5）', async () => {
    useAppStore.setState({ activeProjectId: 'p2' });
    render(<ProjectBar />);

    clickDelete('gamma');
    await screen.findByText('セッション 0 件が一緒に消えます');
    confirmDelete();

    await waitFor(() => expect(deleteProject).toHaveBeenCalledWith('p3'));
    await waitFor(() => expect(useAppStore.getState().projects).toHaveLength(2));
    // remaining = [p1, p2]。remaining[0] は 'p1' なので 'p2' と区別が付く。
    expect(useAppStore.getState().activeProjectId).toBe('p2');
    expect(useAppStore.getState().activeProjectId).not.toBe('p1');
  });

  it('アクティブを削除すると残りの先頭へ落ちる（末尾ではない。契約 §130.5）', async () => {
    render(<ProjectBar />);

    clickDelete('kamux');
    await screen.findByText('セッション 2 件が一緒に消えます');
    confirmDelete();

    await waitFor(() => expect(useAppStore.getState().activeProjectId).toBe('p2'));
    expect(useAppStore.getState().activeProjectId).not.toBe('p3');
    expect(useAppStore.getState().projects.map((p) => p.id)).toEqual(['p2', 'p3']);
    // setActiveProject を通した観測点。盤面・ワークスペース・§85.1 の維持はそこが持つ。
    // 1 往復目は件数のための取得（p1）、2 往復目が盤面の取り直し（p2）である。
    expect(listSessions.mock.calls).toEqual([
      ['p1', true],
      ['p2', true],
    ]);
    expect(localStorage.getItem('kamux.activeProjectId')).toBe('p2');
  });

  it('最後の 1 つを削除すると未選択になり、盤面が空になる（契約 §130.5 / §85.1）', async () => {
    useAppStore.setState({ projects: [project('p1', 'kamux')] });
    render(<ProjectBar />);

    clickDelete('kamux');
    await screen.findByText('セッション 2 件が一緒に消えます');
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
    await screen.findByText('セッション 2 件が一緒に消えます');
    confirmDelete();

    await waitFor(() =>
      expect(screen.queryByRole('dialog', { name: 'プロジェクトを削除' })).toBeNull(),
    );
    // 「消えた」だけを見るとバー自体が描かれていなくても通る。バーが在ることを添える。
    expect(screen.getByRole('button', { name: 'beta を削除' })).toBeTruthy();
  });
});
