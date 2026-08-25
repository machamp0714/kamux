import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { CleanupWorktreeDialogContainer } from './CleanupWorktreeDialogContainer';
import { useAppStore } from '../../store';
import type { Session } from '../../types/model';

// vite.config.ts の test に globals の設定が無いので自動 cleanup は張られない。
afterEach(cleanup);

function session(over: Partial<Session>): Session {
  return {
    id: 's1',
    project_id: 'p1',
    title: 't',
    description: '',
    kanban_status: 'done',
    sort_order: 1,
    mode: 'worktree',
    branch: 'session/fix-login',
    worktree_path: '/repo/a/.worktrees/session-fix-login',
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
    ...over,
  };
}

const closeCleanupDialog = vi.fn();
const confirmCleanup = vi.fn(async () => undefined);
const focusSession = vi.fn();

beforeEach(() => {
  closeCleanupDialog.mockClear();
  confirmCleanup.mockClear();
  focusSession.mockClear();
  useAppStore.setState({
    sessions: { s1: session({}) },
    runtimeStates: {},
    cleanupDialog: null,
    closeCleanupDialog,
    confirmCleanup,
    focusSession,
  });
});

describe('CleanupWorktreeDialogContainer', () => {
  it('cleanupDialog が null なら何も描かない', () => {
    const { container } = render(<CleanupWorktreeDialogContainer />);
    expect(container).toBeEmptyDOMElement();
  });

  it('cleanupDialog のセッションが消えていたら何も描かない', () => {
    useAppStore.setState({
      sessions: {},
      cleanupDialog: { sessionId: 's1', status: null, error: null, busy: false },
    });
    const { container } = render(<CleanupWorktreeDialogContainer />);
    expect(container).toBeEmptyDOMElement();
  });

  it('ダイアログのセッションの worktree_path とブランチを渡す', () => {
    useAppStore.setState({
      sessions: { s1: session({}), s2: session({ id: 's2', branch: 'other', title: 'other' }) },
      cleanupDialog: {
        sessionId: 's1',
        status: { dirty: false, entries: [] },
        error: null,
        busy: false,
      },
    });
    render(<CleanupWorktreeDialogContainer />);

    expect(screen.getByText('/repo/a/.worktrees/session-fix-login')).toBeInTheDocument();
    expect(screen.getByText(/は残ります/)).toHaveTextContent(
      'ブランチ session/fix-login は残ります',
    );
  });

  // 契約 §38.3 論点 2 が却下した形（`runtimeState ?? session.last_runtime_state`）を
  // ここで固定する。純表示コンポーネント側のテストでは、container が `??` を書き戻しても
  // 緑のままになる（props で undefined を直接渡しているだけだから）。
  it('runtimeStates が空なら last_runtime_state で埋めず、稼働中の警告を出さない（契約 §38.3 論点 2）', () => {
    useAppStore.setState({
      sessions: { s1: session({ last_runtime_state: 'running' }) },
      runtimeStates: {},
      cleanupDialog: {
        sessionId: 's1',
        status: { dirty: false, entries: [] },
        error: null,
        busy: false,
      },
    });
    render(<CleanupWorktreeDialogContainer />);

    expect(screen.queryByText(/このセッションはまだ動いています/)).toBeNull();
  });

  it('runtimeStates に running が在れば稼働中の警告を出す', () => {
    useAppStore.setState({
      sessions: { s1: session({ last_runtime_state: 'idle' }) },
      runtimeStates: { s1: 'running' },
      cleanupDialog: {
        sessionId: 's1',
        status: { dirty: false, entries: [] },
        error: null,
        busy: false,
      },
    });
    render(<CleanupWorktreeDialogContainer />);

    expect(screen.getByText(/このセッションはまだ動いています/)).toBeInTheDocument();
  });

  it('削除の確定で confirmCleanup(force) が呼ばれる', () => {
    useAppStore.setState({
      cleanupDialog: {
        sessionId: 's1',
        status: { dirty: true, entries: ['?? a'] },
        error: null,
        busy: false,
      },
    });
    render(<CleanupWorktreeDialogContainer />);

    fireEvent.click(screen.getByLabelText('変更を破棄して強制削除する'));
    fireEvent.click(screen.getByRole('button', { name: '強制削除する' }));

    expect(confirmCleanup).toHaveBeenCalledWith(true);
  });

  it('キャンセルで closeCleanupDialog が呼ばれる', () => {
    useAppStore.setState({
      cleanupDialog: {
        sessionId: 's1',
        status: { dirty: false, entries: [] },
        error: null,
        busy: false,
      },
    });
    render(<CleanupWorktreeDialogContainer />);

    fireEvent.click(screen.getByRole('button', { name: 'キャンセル' }));

    expect(closeCleanupDialog).toHaveBeenCalledTimes(1);
    expect(confirmCleanup).not.toHaveBeenCalled();
  });

  it('「ターミナルで確認する」はダイアログを閉じてから該当セッションへ移す', () => {
    useAppStore.setState({
      cleanupDialog: {
        sessionId: 's1',
        status: { dirty: true, entries: ['?? a'] },
        error: null,
        busy: false,
      },
    });
    render(<CleanupWorktreeDialogContainer />);

    fireEvent.click(screen.getByRole('button', { name: 'ターミナルで確認する' }));

    expect(closeCleanupDialog).toHaveBeenCalledTimes(1);
    expect(focusSession).toHaveBeenCalledWith('s1', 'terminal');
  });

  it('error / busy をそのまま渡す', () => {
    useAppStore.setState({
      cleanupDialog: {
        sessionId: 's1',
        status: { dirty: false, entries: [] },
        error: 'fatal: boom\n',
        busy: true,
      },
    });
    render(<CleanupWorktreeDialogContainer />);

    expect(screen.getByText(/fatal: boom/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '削除する' })).toHaveProperty('disabled', true);
  });
});
