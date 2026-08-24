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

import { getHooksDiagnostics, updateSession } from '../../ipc/commands';
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

  // PR #106 全体レビュー I-1: .cleanup-worktree-dialog__backdrop と .archived-drawer__scrim
  // は同一の --z-scrim を使うため、重なり順は DOM の tree order で決まる（CSS 仕様。同一
  // スタッキングレベルでは後に来る要素が前面）。ドロワーを開いたまま掃除確認ダイアログを
  // 開く導線（ArchivedDrawer の onCleanup）があるため、確認ダイアログが常にドロワーより
  // 前面へ来ることを固定する。マウント順を入れ替える変異はこのテストが無いと全緑になる。
  it('showArchived と cleanupDialog が同時に立つとき、確認ダイアログの backdrop がドロワーの scrim より後ろにマウントされる', () => {
    useAppStore.setState({
      sessions: {
        s1: session({
          mode: 'worktree',
          branch: 'session/fix-login',
          worktree_path: '/repo/a/.worktrees/session-fix-login',
          kanban_status: 'done',
          archived_at: 1754006400000,
        }),
      },
      sessionOrder: { backlog: [], in_progress: [], review: [], done: [] },
      showArchived: true,
      cleanupDialog: {
        sessionId: 's1',
        status: { dirty: false, entries: [] },
        error: null,
        busy: false,
      },
    });

    const { container } = render(<KanbanView />);

    const scrim = container.querySelector('.archived-drawer__scrim');
    const backdrop = container.querySelector('.cleanup-worktree-dialog__backdrop');
    if (scrim === null || backdrop === null) {
      throw new Error(
        'archived-drawer__scrim または cleanup-worktree-dialog__backdrop が見つからない',
      );
    }

    // DOCUMENT_POSITION_FOLLOWING は「backdrop が scrim より後ろ（tree order で後続）」
    // であることを示す。同一 z-index なので、後続であることが前面に来る条件そのもの。
    expect(scrim.compareDocumentPosition(backdrop) & Node.DOCUMENT_POSITION_FOLLOWING).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );
  });
});

// マウントの 1 行（<ArchivedDrawer />）と開閉ボタンの配線が落ちても、ArchivedDrawer
// 単体のテストは単体 render なので全緑のまま通る（裁定 69）。呼び出し側をここで見る。
describe('KanbanView がアーカイブ済みドロワーをマウントする（M3-4 Task 10）', () => {
  it('showArchived が true ならドロワーの中身が現れる', () => {
    useAppStore.setState({
      sessions: {
        s1: session({ archived_at: 1754006400000 }),
      },
      sessionOrder: { backlog: [], in_progress: [], review: [], done: [] },
      showArchived: true,
    });

    render(<KanbanView />);

    expect(screen.getByRole('button', { name: '復元' })).toBeInTheDocument();
  });

  it('「アーカイブ済み」ボタンを押すと showArchived が立つ', () => {
    useAppStore.setState({
      sessions: { s1: session({}) },
      sessionOrder: { backlog: ['s1'], in_progress: [], review: [], done: [] },
      showArchived: false,
    });

    render(<KanbanView />);
    fireEvent.click(screen.getByRole('button', { name: 'アーカイブ済み' }));

    expect(useAppStore.getState().showArchived).toBe(true);
  });

  // onRestore の中身（restoreSession への配線）は ArchivedDrawer.test.tsx では
  // 見えない（あちらは onRestore を vi.fn() で受けている）。ここでしか測れない。
  // レビュー I-2: セッション id をフィクスチャ既定（'s1'）と別の値にし、実装が渡された
  // id をそのまま転送していることを主張する（定数 's1' 固定でも通る形にしない）。
  it('復元ボタンを押すと restoreSession（updateSession の呼び出し）に配線されている', async () => {
    const archived = session({ id: 'sess-restore-7', archived_at: 1754006400000 });
    useAppStore.setState({
      sessions: { 'sess-restore-7': archived },
      sessionOrder: { backlog: [], in_progress: [], review: [], done: [] },
      showArchived: true,
    });
    vi.mocked(updateSession).mockResolvedValue({ ...archived, archived_at: null });

    render(<KanbanView />);
    fireEvent.click(screen.getByRole('button', { name: '復元' }));

    await waitFor(() =>
      expect(updateSession).toHaveBeenCalledWith('sess-restore-7', { archived_at: null }),
    );
  });

  // レビュー Important-2: sessions は project_id === activeProjectId で絞る唯一の
  // 防波堤。restoreSession は target.project_id を見ないので、ここが外れると他
  // プロジェクトのアーカイブ済みが挿入可能な状態でドロワーへ混入する。
  // 修正ラウンド 2 F-1: activeProjectId を beforeEach の既定 'p1' から動かし、
  // 実装が activeProjectId の値を読んでいることを主張する（'p1' 固定でも通る形にしない）。
  it('ドロワーへ渡す sessions はアクティブプロジェクト（p2）だけに絞る', () => {
    useAppStore.setState({
      sessions: {
        s1: session({ id: 's1', title: 'p1 task', project_id: 'p1', archived_at: 1754006400000 }),
        s2: session({ id: 's2', title: 'p2 task', project_id: 'p2', archived_at: 1754006400000 }),
      },
      sessionOrder: { backlog: [], in_progress: [], review: [], done: [] },
      showArchived: true,
      activeProjectId: 'p2',
    });

    render(<KanbanView />);

    expect(screen.getByText('p2 task')).toBeInTheDocument();
    expect(screen.queryByText('p1 task')).toBeNull();
  });

  // レビュー Important-3: 閉じる・掃除の配線 4 経路のうち、container 側の 2 経路
  // （onClose / onCleanup）をここで測る。component 側は ArchivedDrawer.test.tsx。
  it('ドロワーの閉じるボタンを押すと showArchived が false になる', () => {
    useAppStore.setState({
      sessions: { s1: session({ archived_at: 1754006400000 }) },
      sessionOrder: { backlog: [], in_progress: [], review: [], done: [] },
      showArchived: true,
    });

    render(<KanbanView />);
    fireEvent.click(screen.getByRole('button', { name: '閉じる' }));

    expect(useAppStore.getState().showArchived).toBe(false);
  });

  // 修正ラウンド 2 F-2: セッション id をフィクスチャ既定（'s1'）と別の値にし、
  // 実装が渡された id をそのまま転送していることを主張する（定数 's1' 固定でも
  // 通る形にしない）。
  it('掃除ボタンを押すと openCleanupDialog がそのセッション ID で呼ばれる', () => {
    const openCleanupDialog = vi.fn(async () => undefined);
    useAppStore.setState({
      sessions: {
        'sess-cleanup-9': session({
          id: 'sess-cleanup-9',
          mode: 'worktree',
          worktree_path: '/repo/a/.worktrees/session-x',
          archived_at: 1754006400000,
        }),
      },
      sessionOrder: { backlog: [], in_progress: [], review: [], done: [] },
      showArchived: true,
      openCleanupDialog,
    });

    render(<KanbanView />);
    fireEvent.click(screen.getByRole('button', { name: 'worktree を掃除' }));

    expect(openCleanupDialog).toHaveBeenCalledWith('sess-cleanup-9');
  });
});
