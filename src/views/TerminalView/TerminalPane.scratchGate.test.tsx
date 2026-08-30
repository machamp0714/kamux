import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// React 18 の act() は既定でこのフラグを見て、テスト環境かどうかを判定する
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// ゲート修正 A2（brief: .superpowers/sdd/M3-4-ops-ux/gate-fix-brief.md）。
// TerminalPane.test.tsx / TerminalGrid.test.tsx とは対照的に、この 1 ファイルだけは
// terminal/ptyBridge を**モックしない**。A2 の欠陥は「createScratchTerminal が
// markStarted を呼ばないため、TerminalPane のマウント時に isStarted の門を素通りして
// start_session を投げ、バックエンドの二重起動ガードに invalid_state で撥ねられる」
// というモジュールスコープの Set（startedSurfaces）を跨いだ不具合であり、ptyBridge を
// モックすると経路そのものが消えてしまう。terminal/registry も同じ理由で実物を使う
// （ensureTerminal は実 xterm インスタンスを作るが attach していないため fitTerminal は
// null を返し resizePty は呼ばれない。TerminalGrid.test.tsx 以外の多数のテストファイル
// でも同じ理由で jsdom の Canvas 未実装エラーが stderr に出るが、テストの合否には影響しない）。
const mocks = vi.hoisted(() => ({
  createScratchSession: vi.fn(),
  startSession: vi.fn(),
}));

vi.mock('../../ipc/commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../ipc/commands')>()),
  createScratchSession: mocks.createScratchSession,
  startSession: mocks.startSession,
}));

// onPtyData / onPtyExit は listen() 経由で Tauri IPC を叩くため、即座に解決する
// ダミーの unlisten へ差し替える（実際のイベント配送はこのテストの関心ではない）。
vi.mock('../../ipc/events', () => ({
  onPtyData: vi.fn(async () => () => {}),
  onPtyExit: vi.fn(async () => () => {}),
}));

import { useAppStore } from '../../store';
import { resetPtyBridgeForTest, isStarted } from '../../terminal/ptyBridge';
import type { Session } from '../../types/model';
import { surfaceId } from '../../types/model';
import { TerminalPane } from './TerminalPane';

function makeScratchSession(overrides: Partial<Session> & { id: string }): Session {
  return {
    project_id: 'p1',
    title: overrides.id,
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
    heuristics_enabled: false,
    silence_timeout_secs: 30,
    is_scratch: true,
    archived_at: null,
    created_at: 0,
    updated_at: 0,
    ...overrides,
  };
}

async function flush(times = 4): Promise<void> {
  await act(async () => {
    for (let i = 0; i < times; i++) {
      await new Promise<void>((resolve) => queueMicrotask(() => resolve()));
    }
  });
}

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  vi.clearAllMocks();
  resetPtyBridgeForTest();
  useAppStore.setState({
    activeProjectId: 'p1',
    sessions: {},
    sessionOrder: { backlog: [], in_progress: [], review: [], done: [] },
    activePane: 0,
    paneAssignment: [null, null],
    layout: 'single',
  });
  container = document.createElement('div');
  document.body.appendChild(container);
});

afterEach(() => {
  if (root) {
    act(() => {
      root.unmount();
    });
  }
  container.remove();
  resetPtyBridgeForTest();
});

describe('createScratchTerminal → TerminalPane（ゲート修正 A2）', () => {
  it('createScratchTerminal を呼んだ後、そのスクラッチのペインをマウントしても start_session が invoke されない', async () => {
    const created = makeScratchSession({ id: 'scr1' });
    mocks.createScratchSession.mockResolvedValue(created);

    await useAppStore.getState().createScratchTerminal();

    // 根本原因の直接観測: create_scratch_session はバックエンドで spawn 済みなので、
    // createScratchTerminal は markStarted(surfaceId(created.id, 'agent')) を
    // assignPane より前に呼んでいなければならない。
    expect(isStarted(surfaceId('scr1', 'agent'))).toBe(true);

    act(() => {
      root = createRoot(container);
      root.render(<TerminalPane sessionId="scr1" />);
    });
    await flush();

    expect(mocks.startSession).not.toHaveBeenCalled();
  });
});
