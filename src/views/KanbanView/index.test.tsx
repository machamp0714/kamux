import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// store / HooksStatusPanel が読む IPC コマンドをまとめてモックする
// （SessionFormModal.test.tsx と同じ形。store がモジュールロード時に import する）。
vi.mock('../../ipc/commands', () => ({
  createProject: vi.fn(),
  listProjects: vi.fn(),
  createSession: vi.fn(),
  updateSession: vi.fn(),
  listSessions: vi.fn(),
  moveSession: vi.fn(),
  getHooksDiagnostics: vi.fn(),
}));

import { getHooksDiagnostics } from '../../ipc/commands';
import { useAppStore } from '../../store';
import type { Session } from '../../types/model';
import { KanbanView } from './index';

afterEach(cleanup);

function session(overrides: Partial<Session> = {}): Session {
  return {
    id: 's1',
    project_id: 'p1',
    title: 'fix login',
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
    created_at: 0,
    updated_at: 0,
    ...overrides,
  };
}

beforeEach(() => {
  vi.mocked(getHooksDiagnostics).mockReset();
  useAppStore.setState({
    projects: [],
    activeProjectId: 'p1',
    sessions: {},
    modal: null,
  });
});

/** セッション 1 件と、その id を持つ診断をストア／IPC モックに仕込む。 */
function seedOneSession() {
  const s = session({ id: 'sess-42', title: 'fix login' });
  useAppStore.setState({
    sessions: { [s.id]: s },
    sessionOrder: { backlog: [s.id], in_progress: [], review: [], done: [] },
  });
  vi.mocked(getHooksDiagnostics).mockResolvedValue({
    socket_path: '/tmp/kamux-hooks-1234.sock',
    listener_alive: true,
    sessions: [
      {
        session_id: s.id,
        cli_kind: 'shell',
        liveness: 'unreachable',
        last_hook_at: null,
        heuristics_active: true,
      },
    ],
  });
  return s;
}

/** 開閉ボタンを押し、パネルの中身が描画されるまで待つ。 */
async function openDrawer() {
  fireEvent.click(screen.getByRole('button', { name: 'hooks 疎通ステータス' }));
  await waitFor(() => expect(screen.getByTestId('hooks-socket-path')).toBeInTheDocument());
}

/** querySelector の null を非 null アサーション（禁止）なしで潰す。 */
function requireElement(node: Element | null): Element {
  if (node === null) throw new Error('要素が見つからない');
  return node;
}

describe('KanbanView と HooksStatusPanel の統合点', () => {
  it('既定では hooks 疎通ステータスのドロワーは開いていない', () => {
    vi.mocked(getHooksDiagnostics).mockResolvedValue({
      socket_path: '/tmp/kamux-hooks-1234.sock',
      listener_alive: true,
      sessions: [],
    });

    render(<KanbanView />);

    expect(screen.queryByTestId('hooks-socket-path')).toBeNull();
    // 閉じている間はパネルがマウントされないので IPC も呼ばれない（契約 §0）
    expect(getHooksDiagnostics).not.toHaveBeenCalled();
  });

  it('ボタンでドロワーを開くとパネルの中身が現れ、閉じるボタンで消える', async () => {
    // セッションは 1 件だけ置き、その id を診断側の session_id と一致させる。
    // sessionTitles を空オブジェクトに潰す変異はここでタイトルが出ずに落ちる。
    seedOneSession();

    render(<KanbanView />);
    await openDrawer();

    expect(screen.getByTestId('hooks-socket-path').textContent).toContain(
      '/tmp/kamux-hooks-1234.sock',
    );
    expect(screen.getByTestId('hooks-row-sess-42').textContent).toContain('fix login');

    fireEvent.click(screen.getByRole('button', { name: '閉じる' }));

    expect(screen.queryByTestId('hooks-socket-path')).toBeNull();
  });

  it('スクリムを押すとドロワーが閉じる', async () => {
    seedOneSession();

    const { container } = render(<KanbanView />);
    await openDrawer();

    fireEvent.mouseDown(requireElement(container.querySelector('.kanban-view__drawer-scrim')));

    expect(screen.queryByTestId('hooks-socket-path')).toBeNull();
  });

  it('ドロワーは aria-modal を宣言しない（Escape で閉じる手段も focus trap も無いため）', async () => {
    seedOneSession();

    render(<KanbanView />);
    await openDrawer();

    const dialog = screen.getByRole('dialog', { name: 'hooks 疎通ステータス' });
    expect(dialog.getAttribute('aria-modal')).toBeNull();
  });

  it('パネルの中を押してもドロワーは閉じない', async () => {
    // ドロワー本体の stopPropagation が消えると、ソケットパスを選択しようとした
    // 瞬間にスクリムまで mousedown が届いて閉じてしまう。上のスクリムのテストだけ
    // では殺せないので、対で置く。
    seedOneSession();

    render(<KanbanView />);
    await openDrawer();

    fireEvent.mouseDown(screen.getByTestId('hooks-socket-path'));

    expect(screen.getByTestId('hooks-socket-path')).toBeInTheDocument();
  });
});

// マウントの 1 行（<CleanupWorktreeDialogContainer />）が落ちても、ダイアログ側の
// テストも container 側のテストも緑のままになる。呼び出し側をここで見る。
describe('KanbanView が worktree 掃除ダイアログをマウントする（M3-4 Task 9）', () => {
  it('cleanupDialog が立っていれば確認ダイアログが出る', () => {
    useAppStore.setState({
      sessions: {
        s1: session({
          mode: 'worktree',
          branch: 'session/fix-login',
          worktree_path: '/repo/a/.worktrees/session-fix-login',
          kanban_status: 'done',
        }),
      },
      sessionOrder: { backlog: [], in_progress: [], review: [], done: ['s1'] },
      cleanupDialog: {
        sessionId: 's1',
        status: { dirty: false, entries: [] },
        error: null,
        busy: false,
      },
    });

    render(<KanbanView />);

    expect(screen.getByRole('dialog', { name: 'worktree を掃除' })).toBeInTheDocument();
  });

  it('cleanupDialog が null なら確認ダイアログは出ない', () => {
    useAppStore.setState({
      sessions: { s1: session({}) },
      sessionOrder: { backlog: ['s1'], in_progress: [], review: [], done: [] },
      cleanupDialog: null,
    });

    render(<KanbanView />);

    expect(screen.queryByRole('dialog', { name: 'worktree を掃除' })).toBeNull();
  });
});
