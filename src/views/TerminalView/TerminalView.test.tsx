import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// React 18 の act() は既定でこのフラグを見て、テスト環境かどうかを判定する
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const mocks = vi.hoisted(() => ({
  startSession: vi.fn(),
  resizePty: vi.fn(),
  ensurePtySubscription: vi.fn(),
  isStarted: vi.fn(),
  markStarted: vi.fn(),
  unmarkStarted: vi.fn(),
  ensureTerminal: vi.fn(),
  attachTerminal: vi.fn(),
  detachTerminal: vi.fn(),
  fitTerminal: vi.fn(),
  getTerminal: vi.fn(),
  invalidateFitCache: vi.fn(),
  writeNotice: vi.fn(),
}));

vi.mock('../../ipc/commands', () => ({
  startSession: mocks.startSession,
  resizePty: mocks.resizePty,
}));
vi.mock('../../terminal/ptyBridge', () => ({
  ensurePtySubscription: mocks.ensurePtySubscription,
  isStarted: mocks.isStarted,
  markStarted: mocks.markStarted,
  unmarkStarted: mocks.unmarkStarted,
}));
vi.mock('../../terminal/registry', () => ({
  ensureTerminal: mocks.ensureTerminal,
  attachTerminal: mocks.attachTerminal,
  detachTerminal: mocks.detachTerminal,
  fitTerminal: mocks.fitTerminal,
  getTerminal: mocks.getTerminal,
  invalidateFitCache: mocks.invalidateFitCache,
  writeNotice: mocks.writeNotice,
}));

import { useAppStore } from '../../store';
import type { KanbanStatus, Session } from '../../types/model';
import { TerminalView } from './index';

function session(id: string): Session {
  return {
    id,
    project_id: 'p1',
    title: id,
    description: '',
    kanban_status: 'in_progress',
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
    archived_at: null,
    created_at: 0,
    updated_at: 0,
  };
}

class FakeResizeObserver {
  observe(): void {}
  disconnect(): void {}
}

async function flush(times = 4): Promise<void> {
  await act(async () => {
    for (let i = 0; i < times; i++) {
      await new Promise<void>((resolve) => queueMicrotask(() => resolve()));
    }
  });
}

/** フォーカスは requestAnimationFrame 越しに当たるので 1 フレーム進める。 */
async function flushFrame(): Promise<void> {
  await act(async () => {
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
  });
}

/**
 * surface_id をキーに引ける xterm のダミー。`getTerminal` が undefined を返すままだと
 * 「レジストリのキーが session_id に化けた」変異が focus の欠落として現れない。
 */
function fakeTerminals() {
  const s1 = { focus: vi.fn() };
  const s2 = { focus: vi.fn() };
  const terms: Record<string, { focus: () => void }> = { 's1:agent': s1, 's2:agent': s2 };
  mocks.getTerminal.mockImplementation((key: string) => terms[key]);
  return { s1, s2 };
}

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal('ResizeObserver', FakeResizeObserver);
  mocks.isStarted.mockReturnValue(false);
  mocks.ensurePtySubscription.mockResolvedValue(undefined);
  mocks.startSession.mockResolvedValue(undefined);
  mocks.fitTerminal.mockReturnValue(null);
  mocks.getTerminal.mockReturnValue(undefined);

  const emptyOrder: Record<KanbanStatus, string[]> = {
    backlog: [],
    in_progress: ['s1', 's2'],
    review: [],
    done: [],
  };
  useAppStore.setState({
    sessions: { s1: session('s1'), s2: session('s2') },
    sessionOrder: emptyOrder,
    paneAssignment: ['s1', null],
    activePane: 0,
    modal: null,
    view: 'terminal',
    focusedSessionId: null,
  });

  container = document.createElement('div');
  document.body.appendChild(container);
});

afterEach(() => {
  act(() => {
    root.unmount();
  });
  container.remove();
  vi.unstubAllGlobals();
});

describe('TerminalView（不変条件 E・F）', () => {
  it('タブをクリックすると store の paneAssignment が更新され、attachTerminal が新しい surface へ付け替わる', async () => {
    act(() => {
      root = createRoot(container);
      root.render(<TerminalView />);
    });
    await flush();

    expect(mocks.attachTerminal).toHaveBeenCalledWith('s1:agent', expect.any(HTMLElement));

    const tab = container.querySelector('[data-session-id="s2"]');
    expect(tab).not.toBeNull();

    act(() => {
      (tab as HTMLButtonElement).click();
    });
    await flush();

    // E: クリックが assignPane を経由してストアへ反映されたこと
    expect(useAppStore.getState().paneAssignment[0]).toBe('s2');
    // F: 旧 surface は detach され、新 surface へ attach し直されること
    expect(mocks.detachTerminal).toHaveBeenCalledWith('s1:agent');
    expect(mocks.attachTerminal).toHaveBeenCalledWith('s2:agent', expect.any(HTMLElement));
  });
});

describe('TerminalView（要件5: focusedSessionId の xterm へフォーカスする）', () => {
  /**
   * 初期描画では TerminalPane 側の focus effect も同じ surface を引く。
   * 「既にペインに載っているセッションへの再フォーカス」＝ paneAssignment が変わらない
   * 遷移だけを見ることで、TerminalView 自身の effect を分離して検証する
   * （TerminalPane の deps は [sessionId, isActive, modal] なのでこの遷移では再実行されない）。
   */
  async function renderThenReset(terms: ReturnType<typeof fakeTerminals>) {
    act(() => {
      root = createRoot(container);
      root.render(<TerminalView />);
    });
    await flush();
    await flushFrame();
    vi.clearAllMocks();
    terms.s1.focus.mockClear();
    terms.s2.focus.mockClear();
  }

  it('ペインに載ったままのセッションを再フォーカスすると、その surface_id の xterm に focus が当たる', async () => {
    const terms = fakeTerminals();
    await renderThenReset(terms);

    act(() => {
      useAppStore.getState().focusSession('s1', 'terminal');
    });
    await flushFrame();

    // 契約 §16: レジストリのキーは surface_id（session_id ではない）
    expect(mocks.getTerminal).toHaveBeenCalledWith('s1:agent');
    expect(terms.s1.focus).toHaveBeenCalledTimes(1);
  });

  it('モーダル表示中は実シェルへ DOM フォーカスを渡さない', async () => {
    const terms = fakeTerminals();
    await renderThenReset(terms);

    act(() => {
      useAppStore.setState({ modal: { kind: 'create_session' } });
    });
    await flushFrame();
    terms.s1.focus.mockClear();

    act(() => {
      useAppStore.getState().focusSession('s1', 'terminal');
    });
    await flushFrame();

    expect(terms.s1.focus).not.toHaveBeenCalled();
  });
});
