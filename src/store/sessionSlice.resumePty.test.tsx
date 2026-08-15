// 契約 §127.6（PR H2）。「resume_session を invoke する経路が ptyBridge を素通りしている」
// (根)から出る 2 つの症状（S1: 偽の InvalidState / S2: 購読前に emit された pty://data が
// 捨てられる）を、実 ptyBridge のモジュール状態を跨いで守る統合テスト（brief §6.1）。
//
// `../ipc/commands` / `../ipc/events` / `../terminal/registry` はモックするが、
// `../terminal/ptyBridge` は実物を使う ——  markStarted の呼び出し「回数」だけを数える
// テストは、`startedSurfaces` という実際のモジュール状態を跨いだ二重起動の門が
// 本当に効いているかを検証しないため（brief §6.1 の冒頭）。
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const mocks = vi.hoisted(() => ({
  resumeSessionCmd: vi.fn(),
  startSession: vi.fn(),
  resizePty: vi.fn(),
  listSessions: vi.fn(),
  createSession: vi.fn(),
  moveSession: vi.fn(),
  updateSession: vi.fn(),
  onPtyData: vi.fn(),
  onPtyExit: vi.fn(),
  ensureTerminal: vi.fn(),
  writeNotice: vi.fn(),
  writeToTerminal: vi.fn(),
  fitTerminal: vi.fn(),
  invalidateFitCache: vi.fn(),
}));

vi.mock('../ipc/commands', () => ({
  resumeSession: mocks.resumeSessionCmd,
  startSession: mocks.startSession,
  resizePty: mocks.resizePty,
  listSessions: mocks.listSessions,
  createSession: mocks.createSession,
  moveSession: mocks.moveSession,
  updateSession: mocks.updateSession,
}));
vi.mock('../ipc/events', () => ({
  onPtyData: mocks.onPtyData,
  onPtyExit: mocks.onPtyExit,
}));
vi.mock('../terminal/registry', () => ({
  ensureTerminal: mocks.ensureTerminal,
  writeNotice: mocks.writeNotice,
  writeToTerminal: mocks.writeToTerminal,
  fitTerminal: mocks.fitTerminal,
  invalidateFitCache: mocks.invalidateFitCache,
}));

import { useAppStore } from './index';
import { isStarted, resetPtyBridgeForTest } from '../terminal/ptyBridge';
import { surfaceId } from '../types/model';
import type { Session } from '../types/model';
import { TerminalPane } from '../views/TerminalView/TerminalPane';

const SID = '22222222-2222-4222-8222-222222222222';
const SURFACE = surfaceId(SID, 'agent');

function makeSession(overrides: Partial<Session> & { id: string }): Session {
  return {
    project_id: 'p1',
    title: overrides.id,
    description: '',
    kanban_status: 'in_progress',
    sort_order: 1,
    mode: 'worktree',
    branch: null,
    worktree_path: null,
    cli_kind: 'claude',
    cli_command: null,
    claude_session_id: 'existing-claude-session',
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

/** マイクロタスクを複数回消化する（React の関与しない Promise チェーン用）。 */
async function flushMicrotasks(times = 4): Promise<void> {
  for (let i = 0; i < times; i++) {
    await Promise.resolve();
  }
}

let container: HTMLDivElement;
let root: Root;

function renderPane(sessionId: string): void {
  act(() => {
    root = createRoot(container);
    root.render(<TerminalPane sessionId={sessionId} />);
  });
}

/** React effect を挟む Promise チェーンをまとめて消化する */
async function flushPane(times = 4): Promise<void> {
  await act(async () => {
    for (let i = 0; i < times; i++) {
      await new Promise<void>((resolve) => queueMicrotask(() => resolve()));
    }
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  resetPtyBridgeForTest();
  // 既定: listen 登録は即座に解決する（unlisten ハンドルを返す）
  mocks.onPtyData.mockResolvedValue(vi.fn());
  mocks.onPtyExit.mockResolvedValue(vi.fn());
  mocks.fitTerminal.mockReturnValue(null);
  mocks.resizePty.mockResolvedValue(undefined);
  useAppStore.setState({
    sessions: {},
    resumeFailedSessionIds: [],
    runtimeStates: {},
    runtimeReasons: {},
    runtimeErrors: {},
    modal: null,
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
});

describe('resumeSession が ptyBridge へ登録する（契約 §127.6・S1）', () => {
  it('resumeSession 成功後に TerminalPane をマウントしても start_session は呼ばれない', async () => {
    mocks.resumeSessionCmd.mockResolvedValue(makeSession({ id: SID }));

    await useAppStore.getState().resumeSession(SID);

    renderPane(SID);
    await flushPane();

    expect(mocks.startSession).not.toHaveBeenCalled();
    expect(useAppStore.getState().runtimeErrors[SID]).toBeUndefined();
    // 赤い notice（error tone）も書かれない
    expect(mocks.writeNotice).not.toHaveBeenCalled();
  });
});

describe('resumeSession は購読の解決を待つ（契約 §127.6・S2 / 契約 §16）', () => {
  it('ensurePtySubscription が解決するまで resume_session は invoke されない', async () => {
    let resolveData: (handle: () => void) => void = () => {};
    let resolveExit: (handle: () => void) => void = () => {};
    mocks.onPtyData.mockReturnValue(
      new Promise<() => void>((resolve) => {
        resolveData = resolve;
      }),
    );
    mocks.onPtyExit.mockReturnValue(
      new Promise<() => void>((resolve) => {
        resolveExit = resolve;
      }),
    );
    mocks.resumeSessionCmd.mockResolvedValue(makeSession({ id: SID }));

    const pending = useAppStore.getState().resumeSession(SID);
    await flushMicrotasks();
    expect(mocks.resumeSessionCmd).not.toHaveBeenCalled();

    resolveData(vi.fn());
    resolveExit(vi.fn());
    await pending;

    expect(mocks.resumeSessionCmd).toHaveBeenCalledWith(SID);
  });
});

describe('alreadyStarted ガード（契約 §127.6 裁定 A）', () => {
  it('2 回目の resume_session が reject しても isStarted は true のまま', async () => {
    mocks.resumeSessionCmd.mockResolvedValueOnce(makeSession({ id: SID }));
    mocks.resumeSessionCmd.mockRejectedValueOnce({ code: 'io', message: 'boom' });

    await useAppStore.getState().resumeSession(SID);
    expect(isStarted(SURFACE)).toBe(true);

    await expect(useAppStore.getState().resumeSession(SID)).rejects.toBeDefined();
    expect(isStarted(SURFACE)).toBe(true);
  });
});
