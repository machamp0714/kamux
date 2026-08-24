// 契約 §85.2.1 / §86: focusedSessionId === paneAssignment[activePane] という不変条件
// （§85.1）を、`useAppStore.getState()` の関数型メンバ全体（= 組み立て済みストア。
// 契約 §86.1）に対して主張する。2 スライス（terminalSlice / uiSlice）に閉じると
// 3 人目の書き手が sessionSlice / projectSlice に現れた日に原理的に見えなくなるため、
// 母数は必ず `src/store/index.ts` が組み立てた実物のストアから引く（手書き列挙はしない）。
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../ipc/commands', () => ({
  listProjects: vi.fn(),
  createProject: vi.fn(),
  listSessions: vi.fn(),
  createSession: vi.fn(),
  updateSession: vi.fn(),
  moveSession: vi.fn(),
  resumeSession: vi.fn(),
}));

// このファイルの主眼はフォーカス 3 状態の不変条件であり、ptyBridge の実モジュール
// 状態は本ファイルの検証対象ではない（それは sessionSlice.resumePty.test.tsx が
// 実物で担う）。実物のままだと resumeSession('a') が ensureTerminal を実行して
// 実 xterm インスタンスを作ってしまい（起動済み登録簿は本ファイルの関心ではない）、
// jsdom に無い Canvas API のエラーが出力される（契約 §127.6 の brief が明記する
// 既知のハザード）。
vi.mock('../terminal/ptyBridge', () => ({
  ensurePtySubscription: vi.fn(async () => undefined),
  isStarted: vi.fn(() => false),
  markStarted: vi.fn(),
  unmarkStarted: vi.fn(),
}));

import {
  createProject,
  createSession,
  listProjects,
  listSessions,
  moveSession,
  resumeSession,
  updateSession,
  type CreateSessionArgs,
} from '../ipc/commands';
import { useAppStore } from './index';
import { buildSessionOrder } from './kanbanOrder';
import type { PaneAssignment, PaneIndex } from './paneLogic';
import type { Session } from '../types/model';

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

/** §85.1 の不変条件そのもの。 */
function assertInvariant(): void {
  const s = useAppStore.getState();
  expect(s.focusedSessionId).toBe(s.paneAssignment[s.activePane]);
}

interface FocusSnapshot {
  focusedSessionId: string | null;
  paneAssignment: PaneAssignment;
  activePane: PaneIndex;
}

function focusSnapshot(): FocusSnapshot {
  const s = useAppStore.getState();
  return {
    focusedSessionId: s.focusedSessionId,
    paneAssignment: [...s.paneAssignment],
    activePane: s.activePane,
  };
}

// §86.1: 母数はスライス単位で絞らず、組み立て済みストア（src/store/index.ts が
// project / session / ui / terminal の 4 スライスをまとめたもの）の関数型メンバ全体。
// `as const` の配列で持たない（§86.1.1）—— 実物の Object.keys() から毎回導く。
const actionKeys = Object.entries(useAppStore.getState() as unknown as Record<string, unknown>)
  .filter(([, v]) => typeof v === 'function')
  .map(([k]) => k)
  .sort();

// フォーカス 3 状態（focusedSessionId / paneAssignment / activePane）を実際に書きうるアクション。
// これ以外はすべて「呼んでも 3 状態は変化しない」ことを主張する（§86.1.1）。
const FOCUS_TOUCHING_ACTIONS = new Set([
  'focusSession',
  'setLayout',
  'assignPane',
  'setActivePane',
  'cycleSession',
  'resetTerminalLayout',
]);

const CREATE_SESSION_ARGS: CreateSessionArgs = {
  projectId: 'p1',
  title: 'new session',
  description: '',
  mode: 'worktree',
  branch: null,
  cliKind: 'claude',
  cliCommand: null,
};

// 引数表。§86.1.1 の要求どおり、ここに無いキーは「呼ばない」を選んだことになり、
// 直後のテストで actionKeys との集合一致を主張することで黙って飛ばす経路を塞ぐ。
const ARGS: Record<string, unknown[]> = {
  // --- フォーカスを動かしうる 6 アクション ---
  // baseline（後述）は split2 / paneAssignment=['x','y'] / activePane=1。
  // 'x' はペイン 0 に居るセッションなので、routeFocusReducer は
  // 「もう一方のペインに出ている」分岐へ入り activePane だけを動かす
  // （§85.5.1 が要求する経路。変異 #2 の検出はここで担う）。
  focusSession: ['x', 'terminal'],
  // 3 レイアウト目（split2-v）へ実際に遷移させる。
  setLayout: ['split2-v'],
  // baseline に居ないセッションをペイン 0 へ割り当てる（target への実変化）。
  assignPane: [0, 'a'],
  // activePane を 1 → 0 へ実際に動かす。
  setActivePane: [0],
  // タブ順で 1 つ進める（baseline の sessionOrder を使って実際に候補が動く）。
  cycleSession: [1],
  // 「両方 null」で満たす経路（§85.2.1 が名指しで母数に含めるよう要求している）。
  resetTerminalLayout: [],

  // --- フォーカスに触れないアクション（呼んでも 3 状態は不変であることを主張する） ---
  setView: ['kanban'],
  openModal: [{ kind: 'create_session' }],
  closeModal: [],
  setError: [{ code: 'db', message: 'x' }],
  setEditorSurface: ['x', { kind: 'live' }],
  applyStateEvent: [{ session_id: 'x', runtime_state: 'running', reason: 'output_activity' }],
  seedRuntimeStates: [[makeSession({ id: 'x' })], false],
  setRuntimeError: ['x', 'boom'],
  // IPC に触るアクションも母数から外さない（§86.1.1）。セッション削除がフォーカスを
  // 外す経路はまさにここに生えるため、呼べる状態にしてから母数へ入れる。
  loadProjects: [],
  setActiveProject: ['p1'],
  addProject: ['name', '/repo', 'claude'],
  loadSessions: ['p1'],
  addSession: [CREATE_SESSION_ARGS],
  moveCard: ['a', 'review', 0],
  editSession: ['a', { title: 'renamed' }],
  archiveSession: ['a'],
  // 契約 §8 の StateReason::ResumeFailed 経路（M2-4 Task 10）。
  // どちらもフォーカス 3 状態には触れない —— sessions / resumeFailedSessionIds / runtimeErrors だけを書く。
  resumeSession: ['a'],
  retryResumeAsFresh: ['a'],
  // M3-4 Task 8: worktree 掃除ダイアログ。どれも cleanupDialog だけを書き、フォーカス 3 状態には触れない。
  // このファイルは ../ipc/commands を丸ごとモックしており worktreeStatus/cleanupWorktree は
  // 提供されない（undefined 呼び出し）ため、openCleanupDialog/confirmCleanup は catch 経路で
  // 完結する。cleanupDialog は seedBaseline で null のままなので confirmCleanup は早期 return する。
  openCleanupDialog: ['s1'],
  closeCleanupDialog: [],
  confirmCleanup: [false],
};

function seedBaseline(): void {
  const sessions = {
    x: makeSession({ id: 'x', sort_order: 1 }),
    y: makeSession({ id: 'y', sort_order: 2 }),
    a: makeSession({ id: 'a', sort_order: 3 }),
    b: makeSession({ id: 'b', sort_order: 4 }),
  };
  useAppStore.setState({
    projects: [],
    activeProjectId: 'p1',
    sessions,
    sessionOrder: buildSessionOrder(sessions),
    runtimeStates: {},
    runtimeReasons: {},
    runtimeErrors: {},
    view: 'terminal',
    modal: null,
    lastError: null,
    editorSurfaces: {},
    layout: 'split2',
    paneAssignment: ['x', 'y'],
    activePane: 1,
    focusedSessionId: 'y',
  });
}

beforeEach(() => {
  vi.mocked(listProjects).mockReset().mockResolvedValue([]);
  vi.mocked(createProject).mockReset().mockResolvedValue({
    id: 'new-project',
    name: 'n',
    repo_path: '/r',
    default_cli: 'claude',
    created_at: 0,
    updated_at: 0,
  });
  vi.mocked(listSessions).mockReset().mockResolvedValue([]);
  vi.mocked(createSession)
    .mockReset()
    .mockResolvedValue(makeSession({ id: 'created' }));
  vi.mocked(updateSession)
    .mockReset()
    .mockImplementation(async (id, patch) => ({
      ...(useAppStore.getState().sessions[id] ?? makeSession({ id })),
      ...patch,
    }));
  vi.mocked(moveSession)
    .mockReset()
    .mockImplementation(async (id, toStatus) => {
      const target = useAppStore.getState().sessions[id];
      return target ? [{ ...target, kanban_status: toStatus }] : [];
    });
  vi.mocked(resumeSession)
    .mockReset()
    .mockImplementation(async (id) => useAppStore.getState().sessions[id] ?? makeSession({ id }));
  seedBaseline();
});

describe('§85.2.1 / §86: focusedSessionId === paneAssignment[activePane] の不変条件', () => {
  it('引数表 ARGS が母数（組み立て済みストアの関数型メンバ全体）を覆っている', () => {
    // これが導出形の本体（§86.1.1）。新しいアクションが増えた日にこの行が赤くなり、
    // 「呼ぶ」か「呼ばない理由を書く」かを強制する。
    expect(new Set(Object.keys(ARGS))).toEqual(new Set(actionKeys));
  });

  it.each(actionKeys)(
    '%s の実行後も focusedSessionId === paneAssignment[activePane] を保つ',
    async (key) => {
      const before = focusSnapshot();

      const fn = (
        useAppStore.getState() as unknown as Record<string, (...a: unknown[]) => unknown>
      )[key];
      const result = fn(...ARGS[key]);
      if (result instanceof Promise) {
        // IPC 拒否など想定外の reject でテストが落ちないようにする。
        // 不変条件はこの try/catch の外で必ず検証する。
        await result.catch(() => {});
      }

      assertInvariant();

      if (!FOCUS_TOUCHING_ACTIONS.has(key)) {
        // 「呼ばない」を選んだアクションと同じ扱い —— 3 状態が変化しないことを主張する
        // （飛ばすだけでは「書き手ではない」ことの証拠にならない。§86.1.1）。
        expect(focusSnapshot()).toEqual(before);
      }
    },
  );
});

// 棚卸し（3 レイアウト × activePane 0/1 × focusedSessionId null/非 null）。
// terminalSlice.test.ts / uiSlice.test.ts が個々のアクション単位で広くカバーしているが、
// このファイル単体でも routeFocusReducer が主要な分岐をたどることを直接示す。
describe('レイアウト・ペインの組み合わせの棚卸し', () => {
  it('null 始点（single, activePane=0, focusedSessionId=null）でも不変条件を保つ', () => {
    useAppStore.setState({
      layout: 'single',
      paneAssignment: [null, null],
      activePane: 0,
      focusedSessionId: null,
    });

    useAppStore.getState().focusSession('x', 'terminal');

    assertInvariant();
    expect(useAppStore.getState().activePane).toBe(0);
  });

  it('split2-v でも「もう一方のペインに出ている」分岐で activePane が動く', () => {
    useAppStore.setState({
      layout: 'split2-v',
      paneAssignment: ['x', 'y'],
      activePane: 0,
      focusedSessionId: 'x',
    });

    useAppStore.getState().focusSession('y', 'terminal');

    assertInvariant();
    expect(useAppStore.getState().activePane).toBe(1);
    expect(useAppStore.getState().paneAssignment).toEqual(['x', 'y']);
  });

  it('resetTerminalLayout は focusedSessionId と paneAssignment[activePane] をどちらも null にする（両方 null の経路）', () => {
    useAppStore.getState().resetTerminalLayout();

    assertInvariant();
    expect(useAppStore.getState().focusedSessionId).toBeNull();
  });
});
