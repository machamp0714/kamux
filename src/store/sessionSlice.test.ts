import { beforeEach, describe, expect, it, vi } from 'vitest';

const listSessions = vi.fn();
const createSession = vi.fn();
vi.mock('../ipc/commands', () => ({
  listSessions: (...args: unknown[]) => listSessions(...args),
  createSession: (...args: unknown[]) => createSession(...args),
  updateSession: vi.fn(),
  listProjects: vi.fn(),
  createProject: vi.fn(),
}));

import type { Session, SessionStatePayload } from '../types/model';
import { useAppStore } from './index';
import { buildSessionOrder } from './kanbanOrder';
import { emptySessionOrder, indexSessions } from './sessionSlice';

const session = (over: Partial<Session>): Session => ({
  id: 's1',
  project_id: 'p1',
  title: 't',
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
  heuristics_enabled: true,
  silence_timeout_secs: 30,
  archived_at: null,
  created_at: 1,
  updated_at: 1,
  ...over,
});

beforeEach(() => {
  listSessions.mockReset();
  createSession.mockReset();
  // activeProjectId は loadSessions のガードが参照するため、前のテストの値が
  // 漏れないよう毎回リセットする。runtimeStates 系も他 describe との漏れを防ぐため
  // ここでリセットする（各 describe 側にも専用の beforeEach があるが、重複は無害）。
  useAppStore.setState({
    sessions: {},
    sessionOrder: emptySessionOrder(),
    activeProjectId: 'p1',
    runtimeStates: {},
    runtimeReasons: {},
    runtimeErrors: {},
  });
});

describe('emptySessionOrder', () => {
  it('4 列すべてを空配列で持つ', () => {
    expect(emptySessionOrder()).toEqual({ backlog: [], in_progress: [], review: [], done: [] });
  });

  it('呼ぶたびに新しいオブジェクトを返す（列配列を共有しない）', () => {
    const a = emptySessionOrder();
    a.backlog.push('x');
    expect(emptySessionOrder().backlog).toEqual([]);
  });
});

describe('indexSessions', () => {
  it('id 索引と列ごとの sort_order 昇順を作る', () => {
    const { sessions, sessionOrder } = indexSessions([
      session({ id: 'b', kanban_status: 'backlog', sort_order: 2 }),
      session({ id: 'a', kanban_status: 'backlog', sort_order: 1 }),
      session({ id: 'c', kanban_status: 'review', sort_order: 5 }),
    ]);

    expect(Object.keys(sessions).sort()).toEqual(['a', 'b', 'c']);
    expect(sessionOrder.backlog).toEqual(['a', 'b']);
    expect(sessionOrder.review).toEqual(['c']);
    expect(sessionOrder.in_progress).toEqual([]);
    expect(sessionOrder.done).toEqual([]);
  });

  it('空配列でも 4 列を返す', () => {
    expect(indexSessions([]).sessionOrder).toEqual(emptySessionOrder());
  });

  it('sort_order 同値なら id の辞書順にタイブレークする（Task 3 の裁定で委譲）', () => {
    // indexSessions は buildSessionOrder（src/store/kanbanOrder.ts）に委譲している。
    // moveCard の巻き戻し・editSession・archiveSession など sessions マップから
    // 並びを作り直す経路はすべて同じ関数を通るため、id タイブレークが無いと
    // 「リロード直後」と「ローカル再構築後」で同値行の並びが変わり、ユーザーに
    // 見えるちらつきになる（契約 §8.3: create_session の非原子的な採番で同値は
    // 実データに到達する）。ORDER BY の退行検出は Rust 側テストの責務であり
    // （list_sessions の ORDER BY kanban_status, sort_order, id は契約 §17 が固定）、
    // フロントはここで独自にタイブレーク規則を明示して両者を構造で一致させる。
    const { sessionOrder } = indexSessions([
      session({ id: 'z', kanban_status: 'backlog', sort_order: 1 }),
      session({ id: 'a', kanban_status: 'backlog', sort_order: 1 }),
    ]);

    expect(sessionOrder.backlog).toEqual(['a', 'z']);
  });
});

describe('loadSessions', () => {
  it('アーカイブ済みを除いて取得し、ストアに展開する', async () => {
    listSessions.mockResolvedValue([
      session({ id: 'a', sort_order: 2 }),
      session({ id: 'b', sort_order: 1 }),
    ]);

    await useAppStore.getState().loadSessions('p1');

    expect(listSessions).toHaveBeenCalledWith('p1', false);
    expect(useAppStore.getState().sessionOrder.backlog).toEqual(['b', 'a']);
    expect(useAppStore.getState().sessions.a.sort_order).toBe(2);
  });

  it('プロジェクトを切り替えると前のセッションを完全に置き換える', async () => {
    listSessions.mockResolvedValue([session({ id: 'old' })]);
    await useAppStore.getState().loadSessions('p1');

    listSessions.mockResolvedValue([session({ id: 'new', project_id: 'p2' })]);
    // loadSessions は activeProjectId が指すプロジェクトの応答しか適用しない。この行が
    // 無いと、この呼び出し自体がガードの弾く stale 呼び出しになる（lane-controller 裁定）。
    useAppStore.setState({ activeProjectId: 'p2' });
    await useAppStore.getState().loadSessions('p2');

    expect(listSessions).toHaveBeenNthCalledWith(2, 'p2', false);
    expect(Object.keys(useAppStore.getState().sessions)).toEqual(['new']);
    expect(useAppStore.getState().sessionOrder.backlog).toEqual(['new']);
  });

  it('切り替え中に後着した古いプロジェクトの応答で盤面を上書きしない', async () => {
    // A の応答を B より後に解決させる（実際の切り替えで起きる競合そのもの）
    let resolveA: (v: Session[]) => void = () => {};
    listSessions.mockImplementation((projectId: string) => {
      if (projectId === 'pA') {
        return new Promise<Session[]>((r) => {
          resolveA = r;
        });
      }
      return Promise.resolve([session({ id: 'b1', project_id: 'pB' })]);
    });

    const pendingA = useAppStore.getState().loadSessions('pA');
    useAppStore.setState({ activeProjectId: 'pB' });
    await useAppStore.getState().loadSessions('pB');

    // ここで A が後着する
    resolveA([session({ id: 'a1', project_id: 'pA' })]);
    await pendingA;

    expect(useAppStore.getState().sessionOrder.backlog).toEqual(['b1']);
    expect(useAppStore.getState().sessions.a1).toBeUndefined();
  });

  it('要求時のプロジェクトが選択されたままなら通常どおり反映する', async () => {
    useAppStore.setState({ activeProjectId: 'pA' });
    listSessions.mockResolvedValue([session({ id: 'a1', project_id: 'pA' })]);

    await useAppStore.getState().loadSessions('pA');

    expect(useAppStore.getState().sessionOrder.backlog).toEqual(['a1']);
  });

  it('setActiveProject でプロジェクトを切り替えた際も後着応答で上書きしない', async () => {
    // 実際の呼び出し経路（ProjectBar → setActiveProject → loadSessions）で
    // 不変条件が守られることを固定する。
    let resolveA: (v: Session[]) => void = () => {};
    listSessions.mockImplementation((projectId: string) => {
      if (projectId === 'pA') {
        return new Promise<Session[]>((r) => {
          resolveA = r;
        });
      }
      return Promise.resolve([session({ id: 'b1', project_id: 'pB' })]);
    });

    const pendingA = useAppStore.getState().setActiveProject('pA');
    // pA の応答が返る前に pB へ切り替える
    await useAppStore.getState().setActiveProject('pB');

    // ここで pA が後着する
    resolveA([session({ id: 'a1', project_id: 'pA' })]);
    await pendingA;

    expect(useAppStore.getState().sessionOrder.backlog).toEqual(['b1']);
    expect(useAppStore.getState().sessions.a1).toBeUndefined();
  });

  // 必達 1: seedRuntimeStates は isStillActiveProject のガードの下でなければならない。
  // ガードより上に置くと、stale なプロジェクト(pA)の応答だけで runtimeStates が
  // reset=true で作り直され、盤面(sessions/sessionOrder)は pB のまま runtimeStates だけ
  // pA 由来という split-brain になる。
  it('runtimeStates も stale なプロジェクトの応答では書き換えない', async () => {
    let resolveA: (v: Session[]) => void = () => {};
    listSessions.mockImplementation((projectId: string) => {
      if (projectId === 'pA') {
        return new Promise<Session[]>((r) => {
          resolveA = r;
        });
      }
      return Promise.resolve([
        session({ id: 'b1', project_id: 'pB', last_runtime_state: 'idle', first_started_at: 1 }),
      ]);
    });

    const pendingA = useAppStore.getState().loadSessions('pA');
    useAppStore.setState({ activeProjectId: 'pB' });
    await useAppStore.getState().loadSessions('pB');

    // ここで A が後着する
    resolveA([
      session({ id: 'a1', project_id: 'pA', last_runtime_state: 'running', first_started_at: 1 }),
    ]);
    await pendingA;

    expect(useAppStore.getState().runtimeStates).toEqual({ b1: 'idle' });
  });
});

describe('addSession', () => {
  it('作成したセッションを backlog の末尾に足す', async () => {
    listSessions.mockResolvedValue([session({ id: 'a', sort_order: 1 })]);
    await useAppStore.getState().loadSessions('p1');

    // サーバ応答の title を args とあえて異なる値にする。
    // `sessions[created.id]` が「サーバ応答（createSession の戻り値）をそのまま格納」
    // しているのか「args から Session を再構成」しているのかを、この差異で区別できる
    // ようにする（args を転記しただけの実装では created と一致しなくなる）。
    const serverSession = session({ id: 'b', sort_order: 2, title: 'new-from-server' });
    createSession.mockResolvedValue(serverSession);
    const args = {
      projectId: 'p1',
      title: 'new',
      description: '',
      mode: 'in_place' as const,
      branch: null,
      cliKind: 'shell' as const,
      cliCommand: null,
    };
    const created = await useAppStore.getState().addSession(args);

    expect(createSession).toHaveBeenCalledWith(args);
    expect(created).toEqual(serverSession);
    expect(useAppStore.getState().sessionOrder.backlog).toEqual(['a', 'b']);
    // sessions（id 索引）にサーバ応答そのものが格納されていることを検証する。
    expect(useAppStore.getState().sessions.b).toEqual(created);
  });

  it('backlog 以外の列で作られたセッションはその列に足す（backlog 決め打ちを検出する）', async () => {
    const serverSession = session({ id: 'r', kanban_status: 'review', sort_order: 1 });
    createSession.mockResolvedValue(serverSession);

    await useAppStore.getState().addSession({
      projectId: 'p1',
      title: 'r',
      description: '',
      mode: 'in_place',
      branch: null,
      cliKind: 'shell',
      cliCommand: null,
    });

    expect(useAppStore.getState().sessionOrder.review).toEqual(['r']);
    expect(useAppStore.getState().sessionOrder.backlog).toEqual([]);
    // sessions（id 索引）側にも review 列のセッションが入っていることを検証する。
    // sessionOrder だけ更新して sessions を更新し忘れる退行を拾う。
    expect(useAppStore.getState().sessions.r).toEqual(serverSession);
  });

  it('IPC 応答が返るまでにプロジェクトが切り替わっていたら、B の sessions へ A の作成結果を混ぜない（Task 19 の不変条件）', async () => {
    // loadSessions と同じ不変条件（sessions / sessionOrder は常に activeProjectId のもの）を
    // addSession でも守る。IPC 往復中の切り替えは setActiveProject → loadSessions の実経路が
    // そうするように、activeProjectId の更新と sessions / sessionOrder の丸ごと置き換えを伴う。
    useAppStore.setState({ activeProjectId: 'p1' });
    const created = session({ id: 'new', project_id: 'p1' });
    const bSessions = { b: session({ id: 'b', project_id: 'p2', kanban_status: 'review' }) };
    createSession.mockImplementation(async () => {
      useAppStore.setState({
        activeProjectId: 'p2',
        sessions: bSessions,
        sessionOrder: buildSessionOrder(bSessions),
      });
      return created;
    });

    const result = await useAppStore.getState().addSession({
      projectId: 'p1',
      title: 'new',
      description: '',
      mode: 'in_place',
      branch: null,
      cliKind: 'shell',
      cliCommand: null,
    });

    // IPC 自体は成功しているので戻り値は呼び出し元へそのまま返す
    expect(result).toEqual(created);
    // だが stale なプロジェクト(p1)の作成結果を B の sessions へ足してはいけない
    expect(useAppStore.getState().sessions).toEqual(bSessions);
    expect(useAppStore.getState().sessionOrder).toEqual(buildSessionOrder(bSessions));
  });
});

// firstStartedAt の既定は「起動済み」。未起動を試すケースだけ null を明示する（契約 §34.6）
const makeSession = (
  id: string,
  last: Session['last_runtime_state'],
  firstStartedAt: number | null = 1,
): Session => ({
  id,
  project_id: 'p1',
  title: `task ${id}`,
  description: '',
  kanban_status: 'backlog',
  sort_order: 1,
  mode: 'in_place',
  branch: null,
  worktree_path: null,
  cli_kind: 'shell',
  cli_command: null,
  claude_session_id: null,
  last_runtime_state: last,
  last_runtime_error: null,
  first_started_at: firstStartedAt,
  heuristics_enabled: true,
  silence_timeout_secs: 30,
  archived_at: null,
  created_at: 0,
  updated_at: 0,
});

const payload = (
  id: string,
  runtime_state: SessionStatePayload['runtime_state'],
  reason: SessionStatePayload['reason'],
): SessionStatePayload => ({ session_id: id, runtime_state, reason });

describe('sessionSlice runtimeStates', () => {
  beforeEach(() => {
    useAppStore.setState({ runtimeStates: {}, runtimeReasons: {}, runtimeErrors: {} });
  });

  it('seeds runtimeStates from last_runtime_state', () => {
    useAppStore
      .getState()
      .seedRuntimeStates([makeSession('s1', 'interrupted'), makeSession('s2', 'idle')]);
    expect(useAppStore.getState().runtimeStates).toEqual({ s1: 'interrupted', s2: 'idle' });
  });

  it('does not overwrite a live value when seeding', () => {
    useAppStore.getState().applyStateEvent(payload('s1', 'running', 'spawned'));
    useAppStore.getState().seedRuntimeStates([makeSession('s1', 'interrupted')]);
    expect(useAppStore.getState().runtimeStates.s1).toBe('running');
  });

  it('replaces everything when seeding with reset (project switch)', () => {
    useAppStore.getState().applyStateEvent(payload('old', 'running', 'spawned'));
    useAppStore.getState().seedRuntimeStates([makeSession('s1', 'idle')], true);
    expect(useAppStore.getState().runtimeStates).toEqual({ s1: 'idle' });
    expect(useAppStore.getState().runtimeReasons).toEqual({});
  });

  // 契約 §34.6 —— 一度も起動していないセッションは seed しない。
  // runtimeStates[id] が undefined のままになり、RuntimeBadge が null を返す（§33.3 Q1）。
  it('skips never-started sessions when seeding with reset', () => {
    useAppStore
      .getState()
      .seedRuntimeStates([makeSession('fresh', 'idle', null), makeSession('used', 'idle')], true);
    expect(useAppStore.getState().runtimeStates).toEqual({ used: 'idle' });
    expect(useAppStore.getState().runtimeStates.fresh).toBeUndefined();
  });

  // 除外しても、既に持っているエントリまでは消さない
  // （loadSessions のスナップショットが mark_first_started のコミットを追い越した場合）
  it('keeps a live entry for a session whose snapshot has no first_started_at', () => {
    useAppStore.getState().applyStateEvent(payload('s1', 'running', 'spawned'));
    useAppStore.getState().seedRuntimeStates([makeSession('s1', 'idle', null)], true);
    expect(useAppStore.getState().runtimeStates.s1).toBe('running');
  });

  // 非 reset には除外を適用しない（呼び出し元は「たった今起動した」1 件しか渡さない）
  it('seeds a never-started snapshot when reset is not requested', () => {
    useAppStore.getState().seedRuntimeStates([makeSession('s1', 'running', null)]);
    expect(useAppStore.getState().runtimeStates.s1).toBe('running');
  });

  it('applies a state event', () => {
    useAppStore.getState().applyStateEvent(payload('s1', 'waiting_input', 'hook_notification'));
    expect(useAppStore.getState().runtimeStates.s1).toBe('waiting_input');
    expect(useAppStore.getState().runtimeReasons.s1).toBe('hook_notification');
  });

  it('keeps object identity when the event changes nothing', () => {
    useAppStore.getState().applyStateEvent(payload('s1', 'running', 'spawned'));
    const before = useAppStore.getState().runtimeStates;
    useAppStore.getState().applyStateEvent(payload('s1', 'running', 'spawned'));
    expect(useAppStore.getState().runtimeStates).toBe(before);
  });

  it('touches only the target session key', () => {
    useAppStore.getState().applyStateEvent(payload('s1', 'running', 'spawned'));
    useAppStore.getState().applyStateEvent(payload('s2', 'idle', 'hook_stop'));
    expect(useAppStore.getState().runtimeStates).toEqual({ s1: 'running', s2: 'idle' });
  });

  // 設計書 §5.3: runtime_state はカードの列を動かさない
  it('never mutates sessions or sessionOrder', () => {
    useAppStore.setState({
      sessions: { s1: makeSession('s1', 'idle') },
      sessionOrder: { backlog: ['s1'], in_progress: [], review: [], done: [] },
    });
    const beforeSessions = useAppStore.getState().sessions;
    const beforeOrder = useAppStore.getState().sessionOrder;

    useAppStore.getState().applyStateEvent(payload('s1', 'running', 'spawned'));

    expect(useAppStore.getState().sessions).toBe(beforeSessions);
    expect(useAppStore.getState().sessionOrder).toBe(beforeOrder);
    expect(useAppStore.getState().sessions.s1.kanban_status).toBe('backlog');
  });
});

// 契約 §42: 保存した生 stderr をカードが読むための経路。
describe('sessionSlice runtimeErrors', () => {
  const withError = (id: string, message: string | null, firstStartedAt: number | null = 1) => ({
    ...makeSession(id, message === null ? 'idle' : 'error', firstStartedAt),
    last_runtime_error: message,
  });

  beforeEach(() => {
    useAppStore.setState({ runtimeStates: {}, runtimeReasons: {}, runtimeErrors: {} });
  });

  // 契約 §42.3 規約 1
  it('seeds runtimeErrors from last_runtime_error and skips nulls', () => {
    useAppStore
      .getState()
      .seedRuntimeStates(
        [withError('s1', 'claude: command not found\n'), withError('s2', null)],
        true,
      );
    expect(useAppStore.getState().runtimeErrors).toEqual({ s1: 'claude: command not found\n' });
  });

  // 契約 §42.3 規約 2 —— §40.5 の 'error' 例外は runtimeErrors にも効く。
  // これが無いと再起動後に ❌ の枠だけが描かれて中身が空になる。
  it('seeds the error of a never-started session on reset', () => {
    useAppStore.getState().seedRuntimeStates([withError('s1', 'boom', null)], true);
    expect(useAppStore.getState().runtimeStates.s1).toBe('error');
    expect(useAppStore.getState().runtimeErrors.s1).toBe('boom');
  });

  // 契約 §42.3 規約 3 —— DB 側の §17 と同じ規則をストアに写したもの。
  it('clears the error when the session leaves the error state', () => {
    useAppStore.getState().setRuntimeError('s1', 'boom');
    useAppStore.getState().applyStateEvent(payload('s1', 'running', 'spawned'));
    expect(useAppStore.getState().runtimeErrors.s1).toBeUndefined();
  });

  it('keeps the error while the session stays in the error state', () => {
    useAppStore.getState().setRuntimeError('s1', 'boom');
    useAppStore.getState().applyStateEvent(payload('s1', 'error', 'spawn_failed'));
    expect(useAppStore.getState().runtimeErrors.s1).toBe('boom');
  });
});
