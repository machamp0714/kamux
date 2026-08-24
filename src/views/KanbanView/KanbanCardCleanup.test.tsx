import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { KanbanCardCleanup } from './KanbanCardCleanup';
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
    archived_at: null,
    created_at: 0,
    updated_at: 0,
    ...over,
  };
}

const openCleanupDialog = vi.fn(async () => undefined);

beforeEach(() => {
  openCleanupDialog.mockClear();
  useAppStore.setState({ sessions: {}, openCleanupDialog });
});

describe('KanbanCardCleanup', () => {
  it('isCleanupSuggested が false のセッションには何も描かない（in_place）', () => {
    useAppStore.setState({ sessions: { s1: session({ mode: 'in_place', worktree_path: null }) } });
    const { container } = render(<KanbanCardCleanup sessionId="s1" />);
    expect(container).toBeEmptyDOMElement();
  });

  it('done でもアーカイブでもない worktree セッションには何も描かない', () => {
    useAppStore.setState({ sessions: { s1: session({ kanban_status: 'in_progress' }) } });
    const { container } = render(<KanbanCardCleanup sessionId="s1" />);
    expect(container).toBeEmptyDOMElement();
  });

  it('ストアに居ないセッションには何も描かない', () => {
    const { container } = render(<KanbanCardCleanup sessionId="s1" />);
    expect(container).toBeEmptyDOMElement();
  });

  it('done の worktree セッションには掃除ボタンを描く', () => {
    useAppStore.setState({ sessions: { s1: session({}) } });
    render(<KanbanCardCleanup sessionId="s1" />);
    expect(screen.getByRole('button', { name: 'worktree を掃除' })).toBeInTheDocument();
  });

  it('アーカイブ済みの worktree セッションにも掃除ボタンを描く', () => {
    useAppStore.setState({
      sessions: { s1: session({ kanban_status: 'backlog', archived_at: 100 }) },
    });
    render(<KanbanCardCleanup sessionId="s1" />);
    expect(screen.getByRole('button', { name: 'worktree を掃除' })).toBeInTheDocument();
  });

  // sessionId prop と store から引いた session.id は導出関係にあり、取り違えても
  // コードを読んで気づけない。呼ばれた引数を具体値で固定する（変異検証の対象）。
  it('押すと押したカードのセッション ID で openCleanupDialog が呼ばれる', () => {
    useAppStore.setState({
      sessions: { s1: session({}), s2: session({ id: 's2', title: 'other' }) },
    });
    render(<KanbanCardCleanup sessionId="s2" />);

    fireEvent.click(screen.getByRole('button', { name: 'worktree を掃除' }));

    expect(openCleanupDialog).toHaveBeenCalledWith('s2');
  });

  it('クリックはカードの onClick へバブリングしない（stopPropagation）', () => {
    useAppStore.setState({ sessions: { s1: session({}) } });
    const onCardClick = vi.fn();
    render(
      <div onClick={onCardClick}>
        <KanbanCardCleanup sessionId="s1" />
      </div>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'worktree を掃除' }));

    expect(onCardClick).not.toHaveBeenCalled();
  });
});
