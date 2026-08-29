import { beforeEach, describe, expect, it, vi } from 'vitest';

const createScratchSession = vi.fn();
const stopSession = vi.fn();
const updateSession = vi.fn();
vi.mock('../ipc/commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../ipc/commands')>()),
  createScratchSession: (...a: unknown[]) => createScratchSession(...a),
  stopSession: (...a: unknown[]) => stopSession(...a),
  updateSession: (...a: unknown[]) => updateSession(...a),
}));

import { useAppStore } from './index';
import { selectTerminalTabs } from './terminalSlice';
import { isStarted, resetPtyBridgeForTest } from '../terminal/ptyBridge';
import type { Session } from '../types/model';
import { surfaceId } from '../types/model';

const s = (over: Partial<Session> & { id: string }): Session => ({
  project_id: 'p1',
  title: over.id,
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
  first_started_at: 1,
  heuristics_enabled: false,
  silence_timeout_secs: 30,
  is_scratch: false,
  archived_at: null,
  created_at: 0,
  updated_at: 0,
  ...over,
});

// 契約 §29.3 / §29.8、Ruling 20-A〜20-F（lane-controller の裁定）。
// Cmd+T / Cmd+W（src/hooks/useKeymap.ts）が呼ぶストアアクションは sessionSlice.ts に
// 置く（addSession / archiveSession と同じ、IPC を呼ぶセッション生存周期の並び）。
describe('createScratchTerminal（契約 §29.3 / §29.8。Cmd+T が呼ぶ）', () => {
  beforeEach(() => {
    createScratchSession.mockReset();
    resetPtyBridgeForTest();
    useAppStore.setState({
      activeProjectId: 'p1',
      sessions: {},
      sessionOrder: { backlog: [], in_progress: [], review: [], done: [] },
      activePane: 0,
      paneAssignment: [null, null],
      layout: 'single',
    });
  });

  it('createScratchSession(projectId, null) を呼び、戻り値を sessions へ入れる（sessionOrder には入れない。Ruling 20-F）', async () => {
    const created = s({ id: 'scr1', is_scratch: true });
    createScratchSession.mockResolvedValue(created);

    await useAppStore.getState().createScratchTerminal();

    expect(createScratchSession).toHaveBeenCalledWith('p1', null);
    expect(useAppStore.getState().sessions.scr1).toEqual(created);
    // 契約 §29.4: sessionOrder はスクラッチを含まない。
    expect(useAppStore.getState().sessionOrder).toEqual({
      backlog: [],
      in_progress: [],
      review: [],
      done: [],
    });
  });

  it('assignPane(activePane, created.id) を呼ぶ（Ruling 20-F）', async () => {
    const created = s({ id: 'scr1', is_scratch: true });
    createScratchSession.mockResolvedValue(created);
    const assignPane = vi.fn();
    useAppStore.setState({ assignPane, activePane: 1 });

    await useAppStore.getState().createScratchTerminal();

    // activePane と sessionId は同じ「取り違えても素朴なテストは通る」形の値では
    // ないが、引数の順序を保つために具体値で固定する。
    expect(assignPane).toHaveBeenCalledWith(1, 'scr1');
  });

  // Task 20 修正ラウンド 2（team-lead 追加義務）: terminalSlice.test.ts の SCRATCH 系
  // テストは sessions へ scratch の Session をフィクスチャで直置きして
  // selectTerminalTabs を呼ぶだけで、「createScratchTerminal が実際に作った Session が
  // タブへ出る」ことは誰も見ていなかった。ここでは sessions / sessionOrder を一切
  // 直置きせず（beforeEach のリセットのみ）、createScratchSession のモック戻り値だけを
  // 経路にして createScratchTerminal を実行し、その結果を selectTerminalTabs へ渡す。
  it('createScratchTerminal が作った Session は selectTerminalTabs の SCRATCH 経路（契約 §29.7 / Ruling 20-G）に現れる', async () => {
    const created = s({ id: 'scr1', is_scratch: true });
    createScratchSession.mockResolvedValue(created);

    await useAppStore.getState().createScratchTerminal();

    expect(selectTerminalTabs(useAppStore.getState())).toEqual(['scr1']);
  });

  // ゲート修正 A2（PR 33 人間ゲート）: create_scratch_session はバックエンドで既に
  // spawn 済みのため、markStarted を呼ばずに assignPane すると TerminalPane のマウントが
  // isStarted の門を素通りして start_session を投げ、二重起動ガードに invalid_state で
  // 撥ねられる（症状の再現は TerminalPane.scratchGate.test.tsx）。ここでは根本原因の
  // 直接観測として、markStarted(surfaceId(created.id, 'agent')) が呼ばれ、実物の
  // ptyBridge の isStarted がそのサーフェスに対して true を返すことを固定する。
  it('markStarted(surfaceId(created.id, "agent")) を呼ぶ（A2: 二重起動ガードの誤検出を防ぐ）', async () => {
    const created = s({ id: 'scr1', is_scratch: true });
    createScratchSession.mockResolvedValue(created);

    await useAppStore.getState().createScratchTerminal();

    expect(isStarted(surfaceId('scr1', 'agent'))).toBe(true);
    // 取り違え防止: 別 id や別 surface kind ではフラグが立っていないこと。
    expect(isStarted(surfaceId('scr1', 'editor'))).toBe(false);
    expect(isStarted(surfaceId('other', 'agent'))).toBe(false);
  });

  it('アクティブプロジェクトが無ければ何も呼ばない（Ruling 20-E）', async () => {
    useAppStore.setState({ activeProjectId: null });

    await useAppStore.getState().createScratchTerminal();

    expect(createScratchSession).not.toHaveBeenCalled();
  });
});

describe('closeScratchTerminal（契約 §29.3 / §29.8。Cmd+W が呼ぶ）', () => {
  beforeEach(() => {
    stopSession.mockReset();
    updateSession.mockReset();
  });

  it('フォーカス中ペインが scratch のとき stopSession → updateSession({archived_at}) の順に呼ぶ（Ruling 20-B）', async () => {
    const target = s({ id: 'scr1', is_scratch: true });
    useAppStore.setState({
      sessions: { scr1: target },
      sessionOrder: { backlog: [], in_progress: [], review: [], done: [] },
      focusedSessionId: 'scr1',
      activeProjectId: 'p1',
      // Ruling 20-D（Cmd+W 後に paneAssignment を触らない）を、スクラッチ限定の門
      // （Ruling 20-C）を通り抜けた本体実行経路で観測するための baseline。
      // paneInvariant.test.ts の FOCUS_TOUCHING_ACTIONS 側は baseline のフォーカス
      // 中セッションが非 scratch なので早期 return しか測れない
      // （task-20 レビュー Important 2）。ここは focusedSessionId が scratch なので
      // 本体（stopSession → archiveSession）を実行してから below で不変を確認する。
      paneAssignment: ['scr1', null],
      activePane: 0,
    });
    const calls: string[] = [];
    stopSession.mockImplementation(async (id: string) => {
      calls.push(`stop:${id}`);
      return target;
    });
    updateSession.mockImplementation(async (id: string, patch: Record<string, unknown>) => {
      calls.push(`update:${id}`);
      return { ...target, ...patch };
    });

    await useAppStore.getState().closeScratchTerminal();

    // 呼び出し順そのものを固定する（toHaveBeenCalledWith の 2 本だけでは、
    // production 側で 2 行を入れ替えても両方緑のまま通ってしまう）。
    expect(calls).toEqual(['stop:scr1', 'update:scr1']);
    expect(stopSession).toHaveBeenCalledWith('scr1');
    const [id, patch] = updateSession.mock.calls[0] as [string, { archived_at: number }];
    expect(id).toBe('scr1');
    expect(typeof patch.archived_at).toBe('number');

    // Ruling 20-D: archive 後も paneAssignment / focusedSessionId は変化しない
    // （アーカイブ済みの scratch を指し続ける。タブ列からは消えるがペインには残る）。
    expect(useAppStore.getState().paneAssignment).toEqual(['scr1', null]);
    expect(useAppStore.getState().activePane).toBe(0);
    expect(useAppStore.getState().focusedSessionId).toBe('scr1');
  });

  it('フォーカス中ペインが非 scratch のとき stopSession も updateSession も呼ばない（Ruling 20-C の門）', async () => {
    const target = s({ id: 'real1', is_scratch: false });
    useAppStore.setState({
      sessions: { real1: target },
      sessionOrder: { backlog: [], in_progress: [], review: [], done: ['real1'] },
      focusedSessionId: 'real1',
      activeProjectId: 'p1',
    });

    await useAppStore.getState().closeScratchTerminal();

    expect(stopSession).not.toHaveBeenCalled();
    expect(updateSession).not.toHaveBeenCalled();
  });

  it('フォーカス中ペインが空（null）のとき何も呼ばない', async () => {
    useAppStore.setState({
      sessions: {},
      sessionOrder: { backlog: [], in_progress: [], review: [], done: [] },
      focusedSessionId: null,
      activeProjectId: 'p1',
    });

    await useAppStore.getState().closeScratchTerminal();

    expect(stopSession).not.toHaveBeenCalled();
    expect(updateSession).not.toHaveBeenCalled();
  });
});
