import { beforeEach, describe, expect, it, vi } from 'vitest';

const invoked: Array<{ cmd: string; args: Record<string, unknown> }> = [];

vi.mock('../ipc/commands', () => ({
  listSessions: vi.fn(),
  createSession: vi.fn(),
  moveSession: vi.fn(),
  resumeSession: vi.fn(async (id: string) => {
    invoked.push({ cmd: 'resume_session', args: { id } });
    return { id, claude_session_id: null };
  }),
  updateSession: vi.fn(async (id: string, patch: Record<string, unknown>) => {
    invoked.push({ cmd: 'update_session', args: { id, patch } });
    return { id, claude_session_id: null };
  }),
}));

// このファイルの主眼は resumeFailedSessionIds / runtimeErrors の遷移であり、
// ptyBridge の実モジュール状態（起動済み登録簿）は本ファイルの検証対象ではない
// （それは sessionSlice.resumePty.test.tsx が実物で担う）。実物を import すると
// ptyBridge.ts → registry.ts → @xterm/xterm が読み込まれ、かつ ensurePtySubscription が
// 実際の Tauri IPC（window.__TAURI_INTERNALS__）を呼ぼうとして例外になる
// （契約 §127.6 の brief が明記する既知のハザード）ため、ここでは無害化する。
vi.mock('../terminal/ptyBridge', () => ({
  ensurePtySubscription: vi.fn(async () => undefined),
  isStarted: vi.fn(() => false),
  markStarted: vi.fn(),
  unmarkStarted: vi.fn(),
}));

import { useAppStore } from './index';

const SID = '11111111-1111-4111-8111-111111111111';

describe('resume の失敗ハンドリング', () => {
  beforeEach(() => {
    invoked.length = 0;
    useAppStore.setState({
      resumeFailedSessionIds: [],
      runtimeStates: {},
      runtimeReasons: {},
      runtimeErrors: {},
      sessions: {},
    });
  });

  it('reason が resume_failed のとき失敗リストに積む', () => {
    useAppStore.getState().applyStateEvent({
      session_id: SID,
      runtime_state: 'exited',
      reason: 'resume_failed',
    });
    expect(useAppStore.getState().resumeFailedSessionIds).toEqual([SID]);
  });

  it('通常の pty_exited では失敗リストに積まない', () => {
    useAppStore.getState().applyStateEvent({
      session_id: SID,
      runtime_state: 'exited',
      reason: 'pty_exited',
    });
    expect(useAppStore.getState().resumeFailedSessionIds).toEqual([]);
  });

  it('再開に成功したら失敗リストから外れる', async () => {
    useAppStore.setState({ resumeFailedSessionIds: [SID] });
    await useAppStore.getState().resumeSession(SID);
    expect(useAppStore.getState().resumeFailedSessionIds).toEqual([]);
    expect(useAppStore.getState().sessions[SID]).toEqual({ id: SID, claude_session_id: null });
  });

  // ゲート修正 D-1（brief item 8）: resume_session も start_session の双子であり
  // （契約 §123.3）、commit_started_session を通る（applyStartedSession の共有）。
  // Backlog に居るセッションを再開すると sessionOrder が backlog のまま残る不具合の
  // 回帰テスト。
  it('resumeSession が成功すると sessionOrder が更新される（かつ resumeFailedSessionIds から除かれる）', async () => {
    const { resumeSession } = await import('../ipc/commands');
    const resumedSession = {
      id: SID,
      project_id: 'p1',
      title: SID,
      description: '',
      kanban_status: 'in_progress',
      sort_order: 1,
      mode: 'worktree',
      branch: 'feat/resumed',
      worktree_path: '/tmp/resumed',
      cli_kind: 'claude',
      cli_command: null,
      claude_session_id: 'cs1',
      last_runtime_state: 'running',
      last_runtime_error: null,
      first_started_at: 1,
      heuristics_enabled: true,
      silence_timeout_secs: 30,
      is_scratch: false,
      archived_at: null,
      created_at: 0,
      updated_at: 0,
    };
    vi.mocked(resumeSession).mockResolvedValueOnce(resumedSession as never);
    useAppStore.setState({
      activeProjectId: 'p1',
      resumeFailedSessionIds: [SID],
      sessionOrder: { backlog: [SID], in_progress: [], review: [], done: [] },
    });

    await useAppStore.getState().resumeSession(SID);

    expect(useAppStore.getState().sessionOrder).toEqual({
      backlog: [],
      in_progress: [SID],
      review: [],
      done: [],
    });
    expect(useAppStore.getState().resumeFailedSessionIds).toEqual([]);
  });

  // running は resume_failed より必ず先に届く（Spawned → 失敗なら exited/resume_failed の順）。
  // resumeSession の成功以外の経路（例: TerminalPane の直接起動）でも古い失敗フラグが
  // 消えるよう、running を受けたら先に落とす。失敗すれば直後の exited/resume_failed が積み直す。
  it('running を経由すれば resume_failed の再送でも最終的に積まれた状態になる', () => {
    useAppStore.getState().applyStateEvent({
      session_id: SID,
      runtime_state: 'exited',
      reason: 'resume_failed',
    });
    useAppStore.getState().applyStateEvent({
      session_id: SID,
      runtime_state: 'running',
      reason: 'spawned',
    });
    useAppStore.getState().applyStateEvent({
      session_id: SID,
      runtime_state: 'exited',
      reason: 'resume_failed',
    });
    expect(useAppStore.getState().resumeFailedSessionIds).toEqual([SID]);
  });

  it('resume_failed の後に running を受けると失敗リストから外れる', () => {
    useAppStore.getState().applyStateEvent({
      session_id: SID,
      runtime_state: 'exited',
      reason: 'resume_failed',
    });
    useAppStore.getState().applyStateEvent({
      session_id: SID,
      runtime_state: 'running',
      reason: 'spawned',
    });
    expect(useAppStore.getState().resumeFailedSessionIds).toEqual([]);
  });

  // 恒等ガードの !resumeFailedRemoved 項そのものを守る観測（修正ラウンド2）。
  // runtimeStates/runtimeReasons が「同値のまま」running を再送する経路を
  // そのまま踏む —— ここが同値でなければ他の項だけで早期 return を回避できてしまい、
  // !resumeFailedRemoved の項が在っても無くても緑になる（観測にならない）。
  // 同値であることがこのテストの本体である。
  it('runtimeStates/runtimeReasons が同値のまま running を再送しても resumeFailedSessionIds は外れる', () => {
    useAppStore.getState().applyStateEvent({
      session_id: SID,
      runtime_state: 'running',
      reason: 'spawned',
    });
    // resumeFailedSessionIds だけを「載っている」状態へ差し替える。
    // runtimeStates / runtimeReasons は直前の running のまま据え置く。
    useAppStore.setState({ resumeFailedSessionIds: [SID] });

    // 同値の running を再送する。state/reason は変わらないので、恒等ガードを
    // 抜けられるかどうかは resumeFailedRemoved の項だけにかかっている。
    useAppStore.getState().applyStateEvent({
      session_id: SID,
      runtime_state: 'running',
      reason: 'spawned',
    });

    expect(useAppStore.getState().resumeFailedSessionIds).toEqual([]);
  });

  it('retryResumeAsFresh は claude_session_id をクリアしてから再開する', async () => {
    await useAppStore.getState().retryResumeAsFresh(SID);
    expect(invoked).toEqual([
      { cmd: 'update_session', args: { id: SID, patch: { claude_session_id: null } } },
      { cmd: 'resume_session', args: { id: SID } },
    ]);
  });

  // クリアは成功したが再開が失敗した場合、ストアが古い claude_session_id を
  // 持ったままだとカードのラベルが「会話を再開」に戻り、DB と食い違う。
  it('再開が失敗してもクリア結果はストアに反映されている', async () => {
    const { resumeSession } = await import('../ipc/commands');
    vi.mocked(resumeSession).mockRejectedValueOnce({ code: 'io', message: 'boom' });
    useAppStore.setState({
      sessions: { [SID]: { id: SID, claude_session_id: 'stale-id' } as never },
    });

    await expect(useAppStore.getState().retryResumeAsFresh(SID)).rejects.toBeDefined();
    expect(useAppStore.getState().sessions[SID].claude_session_id).toBeNull();
  });

  // 契約 §42.3 規約 4 と同じ場所（setRuntimeError）に落ちること。呼ばれた「こと」だけでなく
  // 渡す文字列が toAppError(e).message そのものであることまで固定する（取り違え防止）。
  it('再開が失敗したら runtimeErrors にそのエラーメッセージを残す', async () => {
    const { resumeSession } = await import('../ipc/commands');
    vi.mocked(resumeSession).mockRejectedValueOnce({ code: 'io', message: 'resume boom' });

    await expect(useAppStore.getState().resumeSession(SID)).rejects.toBeDefined();
    expect(useAppStore.getState().runtimeErrors[SID]).toBe('resume boom');
  });

  // 恒等ガードは resumeFailedSessionIds も所有する。4 つの項が全部同値なら
  // runtimeStates と resumeFailedSessionIds のどちらも参照を変えない。
  it('4 つの項が全部同値なら resumeFailedSessionIds の参照も変わらない', () => {
    useAppStore.getState().applyStateEvent({
      session_id: SID,
      runtime_state: 'exited',
      reason: 'resume_failed',
    });
    const beforeStates = useAppStore.getState().runtimeStates;
    const beforeFailed = useAppStore.getState().resumeFailedSessionIds;

    useAppStore.getState().applyStateEvent({
      session_id: SID,
      runtime_state: 'exited',
      reason: 'resume_failed',
    });

    expect(useAppStore.getState().runtimeStates).toBe(beforeStates);
    expect(useAppStore.getState().resumeFailedSessionIds).toBe(beforeFailed);
  });

  // resumeFailedSessionIds だけが変わったとき（state/reason は同値のまま）にも
  // 恒等ガードが正しく「更新した」と判定すること。再開成功でリストから外れた後、
  // 同じ resume_failed イベントが再送される経路（実在: バックエンドの再送 / 競合）を模す。
  it('state/reason が同値でも resumeFailedSessionIds がリセットされていれば再送で積み直す', () => {
    useAppStore.getState().applyStateEvent({
      session_id: SID,
      runtime_state: 'exited',
      reason: 'resume_failed',
    });
    // 再開成功を模して resumeFailedSessionIds だけを空に戻す
    // （runtimeStates / runtimeReasons は据え置き。resumeSession アクション自体は
    //  runtime_state を書き換えないので、この状態は実際に起こりうる）
    useAppStore.setState({ resumeFailedSessionIds: [] });

    useAppStore.getState().applyStateEvent({
      session_id: SID,
      runtime_state: 'exited',
      reason: 'resume_failed',
    });

    expect(useAppStore.getState().resumeFailedSessionIds).toEqual([SID]);
  });
});
