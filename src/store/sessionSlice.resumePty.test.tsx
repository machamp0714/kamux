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
import { waitFor } from '@testing-library/react';
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
    is_scratch: false,
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
    expect(mocks.onPtyData).toHaveBeenCalledWith(SURFACE, expect.any(Function));
    expect(mocks.onPtyExit).toHaveBeenCalledWith(SURFACE, expect.any(Function));
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

// レビュー I-1（task-1-review.md）: 段 1（ensurePtySubscription）の失敗経路（裁定 B）を、
// これまでどのテストも守っていなかった（S2 のテストは onPtyData/onPtyExit を最終的に
// resolve させるので reject 側を一度も通らない）。段 1 を reject させ、レビューが挙げた
// 5 点 (a)〜(e) を 1 本で観測する。
describe('段 1 の失敗経路（契約 §127.6 裁定 B・レビュー I-1）', () => {
  it('ensurePtySubscription が reject したら resume_session を invoke せず、門も立てず、error tone の notice を書き、runtimeErrors は付けずに reject する', async () => {
    const subscribeError = new Error('listen failed');
    mocks.onPtyData.mockRejectedValue(subscribeError);
    mocks.onPtyExit.mockResolvedValue(vi.fn());

    // (e) resumeSession の Promise が reject する
    await expect(useAppStore.getState().resumeSession(SID)).rejects.toBe(subscribeError);

    // (a) resume_session が 1 度も invoke されない
    expect(mocks.resumeSessionCmd).not.toHaveBeenCalled();
    // (b) isStarted(surface) が false のまま（段 2 を実行していない）
    expect(isStarted(SURFACE)).toBe(false);
    // (c) writeNotice が 'error' tone で呼ばれる
    expect(mocks.writeNotice).toHaveBeenCalledWith(
      SURFACE,
      `PTY イベントの購読に失敗しました: ${String(subscribeError)}`,
      'error',
    );
    // (d) setRuntimeError が呼ばれず runtimeErrors[SID] が付かない
    expect(useAppStore.getState().runtimeErrors[SID]).toBeUndefined();
  });
});

// レビュー I-2（task-1-review.md）: 段 1 と段 2 の「順序」自体が無観測だった（RM-MARK は
// markStarted の存在しか見ていない）。購読の解決前後で isStarted(surface) の値を跨いで
// 観測する。RM-ORDER-A（markStarted を段 1 の await より前へ）・RM-ORDER-B（markStarted を
// invoke 解決後へ）・RM-NOSUB（段 1 を丸ごと撤去）のいずれでも赤くなる。
describe('段 1→段 2 の順序（契約 §127.6・レビュー I-2）', () => {
  it('markStarted は購読解決の後・invoke 解決の前に実行される', async () => {
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
    let resolveInvoke: (session: Session) => void = () => {};
    mocks.resumeSessionCmd.mockReturnValue(
      new Promise<Session>((resolve) => {
        resolveInvoke = resolve;
      }),
    );

    const pending = useAppStore.getState().resumeSession(SID);
    await flushMicrotasks();
    // 購読が未解決の間は markStarted も未実行（段 1 が段 2 より先）
    expect(isStarted(SURFACE)).toBe(false);

    resolveData(vi.fn());
    resolveExit(vi.fn());
    await waitFor(() => expect(mocks.resumeSessionCmd).toHaveBeenCalledWith(SID));
    // invoke が pending の間も isStarted は true（段 2 は invoke 解決を待たずに実行される）
    expect(isStarted(SURFACE)).toBe(true);

    resolveInvoke(makeSession({ id: SID }));
    await pending;
  });
});

// レビュー「範囲外の観察」（task-1-review.md rev1 末尾）: 一度も起動していない
// セッションの再開が失敗した場合に unmarkStarted(surface) が門を戻すことを、
// これまでどのテストも観測していなかった（既存の失敗経路テストは resume.test.ts 側で
// ../terminal/ptyBridge を vi.mock しており isStarted を観測できない）。
describe('段 2 の失敗経路で門を戻す（sessionSlice.ts の unmarkStarted・修正ラウンド 2）', () => {
  it('一度も起動していないセッションの再開が失敗したら isStarted(surfaceId(SID,agent)) が false に戻る', async () => {
    mocks.resumeSessionCmd.mockRejectedValueOnce({ code: 'io', message: 'boom' });

    expect(isStarted(SURFACE)).toBe(false);

    await expect(useAppStore.getState().resumeSession(SID)).rejects.toBeDefined();

    expect(isStarted(SURFACE)).toBe(false);
  });
});

// lane-controller の追加裁定（task-1-brief-round2 相当）: retryResumeAsFresh
// （新しい会話として開始）が resumeSession を経由することで登録を継承することを見る。
describe('retryResumeAsFresh は resumeSession の登録を継承する', () => {
  it('retryResumeAsFresh の後、agent surface の isStarted は true である', async () => {
    mocks.updateSession.mockResolvedValue(makeSession({ id: SID, claude_session_id: null }));
    mocks.resumeSessionCmd.mockResolvedValue(makeSession({ id: SID }));

    await useAppStore.getState().retryResumeAsFresh(SID);

    expect(isStarted(SURFACE)).toBe(true);
  });
});
