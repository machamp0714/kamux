import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// sessionSlice / projectSlice が読む IPC コマンドをまとめてモックする
// （sessionActions.test.ts と同じ形。store がモジュールロード時にこれらを import する）。
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
import { SessionFormModal } from './SessionFormModal';

afterEach(cleanup);

function session(overrides: Partial<Session> = {}): Session {
  return {
    id: 's1',
    project_id: 'p1',
    title: 'fix login',
    description: '',
    kanban_status: 'in_progress',
    sort_order: 1,
    mode: 'in_place',
    branch: null,
    worktree_path: null,
    cli_kind: 'custom',
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
    ...overrides,
  };
}

beforeEach(() => {
  vi.mocked(updateSession).mockReset();
  vi.mocked(getHooksDiagnostics).mockReset();
  // 既定は「診断は取れたが対象セッションの行は無い」= liveness undefined。
  vi.mocked(getHooksDiagnostics).mockResolvedValue({
    socket_path: '/tmp/kamux-hooks-1234.sock',
    listener_alive: true,
    sessions: [],
  });
  useAppStore.setState({
    projects: [],
    activeProjectId: 'p1',
    sessions: {},
    modal: null,
  });
});

describe('SessionFormModal と HeuristicsSettings の統合点', () => {
  it('edit モードでトグルを押すと editSession が正しい引数で呼ばれ、表示が新しい値に追随する', async () => {
    // id はフィクスチャの既定値 's1' を使わない。既定値のままだと
    // editSession(dialogMode.session.id, patch) を editSession('s1', patch) に
    // 潰す変異が緑を通ってしまう（契約 §81 の条件1+2）。
    const s = session({ id: 'sess-42', heuristics_enabled: true });
    useAppStore.setState({
      sessions: { [s.id]: s },
      modal: { kind: 'edit_session', sessionId: s.id },
    });
    vi.mocked(updateSession).mockResolvedValue({ ...s, heuristics_enabled: false });

    render(<SessionFormModal />);

    const toggle = screen.getByLabelText('ヒューリスティック検知');
    expect(toggle).toBeChecked();

    await act(async () => {
      fireEvent.click(toggle);
      await Promise.resolve();
    });

    expect(updateSession).toHaveBeenCalledWith(s.id, { heuristics_enabled: false });

    await waitFor(() => {
      expect(screen.getByLabelText('ヒューリスティック検知')).not.toBeChecked();
    });
  });

  it('hooks が届いているセッションでは検知方式が hooks（確実）になる', async () => {
    // 診断の sessions は 2 件入れ、編集対象を先頭以外に置く。
    // sessions.find(...) を sessions[0] に潰す変異はここで
    // 「ヒューリスティック（推定）」に転んで落ちる（契約 §81 の条件1+2）。
    const s = session({ id: 'sess-42', heuristics_enabled: true });
    useAppStore.setState({
      sessions: { [s.id]: s },
      modal: { kind: 'edit_session', sessionId: s.id },
    });
    vi.mocked(getHooksDiagnostics).mockResolvedValue({
      socket_path: '/tmp/kamux-hooks-1234.sock',
      listener_alive: true,
      sessions: [
        {
          session_id: 'other-1',
          cli_kind: 'claude',
          liveness: 'unreachable',
          last_hook_at: null,
          heuristics_active: true,
        },
        {
          session_id: s.id,
          cli_kind: 'custom',
          liveness: 'healthy',
          last_hook_at: 1,
          heuristics_active: false,
        },
      ],
    });

    render(<SessionFormModal />);

    await waitFor(() => expect(screen.getByText('hooks（確実）')).toBeInTheDocument());
  });

  it('診断に当該セッションの行が無ければ検知方式はヒューリスティック（推定）のまま', async () => {
    const s = session({ id: 'sess-42', heuristics_enabled: true });
    useAppStore.setState({
      sessions: { [s.id]: s },
      modal: { kind: 'edit_session', sessionId: s.id },
    });

    render(<SessionFormModal />);

    await waitFor(() => expect(getHooksDiagnostics).toHaveBeenCalled());
    expect(screen.getByText('ヒューリスティック（推定）')).toBeInTheDocument();
  });

  it('create モードでは HeuristicsSettings が描画されない', () => {
    useAppStore.setState({ modal: { kind: 'create_session' } });

    render(<SessionFormModal />);

    expect(screen.queryByLabelText('ヒューリスティック検知')).toBeNull();
  });
});
