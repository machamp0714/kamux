import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// React 18 の act() は既定でこのフラグを見て、テスト環境かどうかを判定する
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// ゲート修正 A3（PR 108 人間ゲート）。閉じたスクラッチがペインに載ったまま残ると、
// `TerminalGrid` が再マウントされた時点で `TerminalPane` がもう一度描かれ、
// `pty://exit` で開いた二重起動の門（`ptyBridge.ts` の `startedSurfaces.delete`）を
// 通って `start_session` が飛ぶ。UI のどこにも現れない `$SHELL -l` が立ち上がる。
//
// この経路は「モジュールスコープの起動済み登録簿」「ストアの `paneAssignment`」
// 「React の再マウント」の 3 つを跨ぐので、`terminal/ptyBridge` は**モックしない**
// （`TerminalPane.scratchGate.test.tsx` と同じ理由）。`terminal/registry` は
// 実 xterm を jsdom に生やさないためモックする —— 門は ptyBridge 側にあり、
// registry は本テストの観測対象ではない。
const mocks = vi.hoisted(() => ({
  createScratchSession: vi.fn(),
  startSession: vi.fn(),
  stopSession: vi.fn(),
  updateSession: vi.fn(),
  resizePty: vi.fn(),
  /** surfaceId → ptyBridge が登録した `pty://exit` ハンドラ。バックエンドの exit を再現する。 */
  exitHandlers: new Map<string, (p: { surface_id: string; exit_code: number | null }) => void>(),
}));

vi.mock('../../ipc/commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../ipc/commands')>()),
  createScratchSession: mocks.createScratchSession,
  startSession: mocks.startSession,
  stopSession: mocks.stopSession,
  updateSession: mocks.updateSession,
  resizePty: mocks.resizePty,
}));

vi.mock('../../ipc/events', () => ({
  onPtyData: vi.fn(async () => () => {}),
  onPtyExit: vi.fn(
    async (
      surfaceId: string,
      handler: (p: { surface_id: string; exit_code: number | null }) => void,
    ) => {
      mocks.exitHandlers.set(surfaceId, handler);
      return () => {};
    },
  ),
}));

// disposeTerminal は意図的に生やさない（TerminalGrid.test.tsx と同じ、契約 §16 の検出器）。
vi.mock('../../terminal/registry', () => ({
  ensureTerminal: vi.fn(),
  getTerminal: vi.fn(() => undefined),
  attachTerminal: vi.fn(),
  detachTerminal: vi.fn(),
  fitTerminal: vi.fn(() => null),
  invalidateFitCache: vi.fn(),
  writeToTerminal: vi.fn(),
  writeNotice: vi.fn(),
}));

import { useAppStore } from '../../store';
import { isStarted, resetPtyBridgeForTest } from '../../terminal/ptyBridge';
import type { Session } from '../../types/model';
import { surfaceId } from '../../types/model';
import { TerminalGrid } from './TerminalGrid';

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

/** jsdom に ResizeObserver は無い。TerminalGrid の effect が参照するだけなので空実装で足りる。 */
class NoopResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

async function flush(times = 4): Promise<void> {
  await act(async () => {
    for (let i = 0; i < times; i++) {
      await new Promise<void>((resolve) => queueMicrotask(() => resolve()));
    }
  });
}

let container: HTMLDivElement;
let root: Root | null = null;

function mountGrid(): void {
  act(() => {
    root = createRoot(container);
    root.render(<TerminalGrid />);
  });
}

function unmountGrid(): void {
  act(() => {
    root?.unmount();
  });
  root = null;
}

beforeEach(() => {
  vi.clearAllMocks();
  mocks.exitHandlers.clear();
  vi.stubGlobal('ResizeObserver', NoopResizeObserver);
  resetPtyBridgeForTest();
  useAppStore.setState({
    activeProjectId: 'p1',
    sessions: {},
    sessionOrder: { backlog: [], in_progress: [], review: [], done: [] },
    view: 'terminal',
    modal: null,
    layout: 'single',
    paneAssignment: [null, null],
    activePane: 0,
    focusedSessionId: null,
  });
  container = document.createElement('div');
  document.body.appendChild(container);
});

afterEach(() => {
  unmountGrid();
  container.remove();
  resetPtyBridgeForTest();
  vi.unstubAllGlobals();
});

describe('closeScratchTerminal → TerminalGrid 再マウント（ゲート修正 A3）', () => {
  it('Cmd+W で閉じたスクラッチは、グリッドを再マウントしても start_session が invoke されない', async () => {
    const created = makeScratchSession({ id: 'scr1' });
    mocks.createScratchSession.mockResolvedValue(created);
    mocks.stopSession.mockResolvedValue(created);
    mocks.updateSession.mockImplementation(async (_id: string, patch: Record<string, unknown>) => ({
      ...created,
      ...patch,
    }));

    await act(async () => {
      await useAppStore.getState().createScratchTerminal();
    });

    mountGrid();
    await flush();

    // 前提: create_scratch_session はバックエンドで spawn 済みなので、この時点では
    // 門（A2 の修正）が閉じており start_session は飛んでいない。
    expect(mocks.startSession).not.toHaveBeenCalled();
    expect(isStarted(surfaceId('scr1', 'agent'))).toBe(true);

    await act(async () => {
      await useAppStore.getState().closeScratchTerminal();
    });

    // stop_session を受けたバックエンドが返す pty://exit を再現する。ここで
    // ptyBridge の startedSurfaces から消えるため、二重起動の門は開いた状態になる。
    act(() => {
      mocks.exitHandlers.get(surfaceId('scr1', 'agent'))?.({
        surface_id: surfaceId('scr1', 'agent'),
        exit_code: 0,
      });
    });
    expect(isStarted(surfaceId('scr1', 'agent'))).toBe(false);

    // view 切替などでグリッドが作り直される経路（App.tsx の `view === 'terminal' &&`）。
    unmountGrid();
    mountGrid();
    await flush();

    // 門は開いている。それでも start_session が飛ばないのは、閉じたスクラッチが
    // どのペインにも載っていないため TerminalPane が描かれないからである。
    expect(mocks.startSession).not.toHaveBeenCalled();
  });
});
