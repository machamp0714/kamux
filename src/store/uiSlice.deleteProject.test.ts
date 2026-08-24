import { beforeEach, describe, expect, it, vi } from 'vitest';

const listSessions = vi.fn();
vi.mock('../ipc/commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../ipc/commands')>()),
  listSessions: (...a: unknown[]) => listSessions(...a),
}));

import { useAppStore } from './index';
import type { CleanupDialogState } from './cleanup';
import type { Session } from '../types/model';

const cleanup: CleanupDialogState = {
  sessionId: 's1',
  status: null,
  error: null,
  busy: false,
};

const session = (id: string, projectId: string): Session => ({
  id,
  project_id: projectId,
  title: id,
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
});

/** 開いた直後（応答が届く前）の状態。件数はまだ主張しない。 */
const PENDING = { projectId: 'p7', sessions: null, error: null };

describe('deleteProjectDialog', () => {
  beforeEach(() => {
    listSessions.mockReset().mockResolvedValue([]);
    useAppStore.setState({
      deleteProjectDialog: null,
      projectSwitcherOpen: false,
      modal: null,
      cleanupDialog: null,
    });
  });

  it('既定では閉じている', () => {
    expect(useAppStore.getState().deleteProjectDialog).toBeNull();
  });

  it('openDeleteProjectDialog は削除対象のプロジェクトを持って開く', async () => {
    await useAppStore.getState().openDeleteProjectDialog('p7');

    // 取り違えたら別物になる具体値で観測する（開閉フラグではなく対象 id を持つこと）。
    expect(useAppStore.getState().deleteProjectDialog).toEqual({
      projectId: 'p7',
      sessions: [],
      error: null,
    });
  });

  /**
   * 消える件数は `st.sessions` からは引けない —— `loadSessions` は「置換ではなくマージ」で、
   * 一度もアクティブにしていないプロジェクトのセッションは 1 件も載っていない。
   * `includeArchived` は `true`。契約 §3 の `ON DELETE CASCADE` はアーカイブ済みの行も消す
   * ので、消えるものを数えるなら母数に入れる。
   */
  it('openDeleteProjectDialog は対象プロジェクトの list_sessions を includeArchived: true で撃つ', async () => {
    listSessions.mockResolvedValue([session('s1', 'p7'), session('s2', 'p7')]);

    await useAppStore.getState().openDeleteProjectDialog('p7');

    expect(listSessions).toHaveBeenCalledWith('p7', true);
    expect(useAppStore.getState().deleteProjectDialog?.sessions?.map((s) => s.id)).toEqual([
      's1',
      's2',
    ]);
  });

  it('応答が届くまでは件数を持たない（null = 取得中。0 件と書かない）', async () => {
    const pending = useAppStore.getState().openDeleteProjectDialog('p7');

    expect(useAppStore.getState().deleteProjectDialog).toEqual(PENDING);

    await pending;
    expect(useAppStore.getState().deleteProjectDialog?.sessions).toEqual([]);
  });

  it('取得に失敗したら error を持ち、件数は主張しないまま（契約 §6）', async () => {
    listSessions.mockRejectedValue({ code: 'db', message: 'db is locked' });

    await useAppStore.getState().openDeleteProjectDialog('p7');

    expect(useAppStore.getState().deleteProjectDialog).toEqual({
      projectId: 'p7',
      sessions: null,
      error: 'db is locked',
    });
  });

  // 往復中に別のプロジェクトへ開き直したら、古い応答は捨てる（openCleanupDialog と同じ形）。
  it('往復中に対象が変わったら古い応答を適用しない', async () => {
    listSessions.mockImplementation((projectId: string) =>
      projectId === 'p7' ? Promise.resolve([session('old', 'p7')]) : Promise.resolve([]),
    );

    const stale = useAppStore.getState().openDeleteProjectDialog('p7');
    useAppStore.setState({ deleteProjectDialog: { projectId: 'p9', sessions: null, error: null } });
    await stale;

    expect(useAppStore.getState().deleteProjectDialog).toEqual({
      projectId: 'p9',
      sessions: null,
      error: null,
    });
  });

  // 上のテスト（成功側）の鏡像（catch 側）。`await listSessions` が reject する経路にも
  // 同じガード（`st.deleteProjectDialog?.projectId === projectId`）があり、往復中に
  // 対象が変わっていたら古い reject を適用してはいけない（先例は
  // `uiSlice.overlayExclusion.test.ts` の
  // 'openCleanupDialog の遅延応答（catch 側）は、往復中に対象がずれたら modal に触れない'）。
  it('往復中に対象が変わったら古い応答（catch 側）を適用しない', async () => {
    let rejectSessions: (e: { code: string; message: string }) => void = () => {};
    listSessions.mockImplementation(
      () =>
        new Promise<Session[]>((_resolve, reject) => {
          rejectSessions = reject;
        }),
    );

    const stale = useAppStore.getState().openDeleteProjectDialog('p7');
    // 往復中に対象が 'p9' へ切り替わる。
    useAppStore.setState({ deleteProjectDialog: { projectId: 'p9', sessions: null, error: null } });

    rejectSessions({ code: 'git', message: 'stale error' });
    await stale;

    expect(useAppStore.getState().deleteProjectDialog).toEqual({
      projectId: 'p9',
      sessions: null,
      error: null,
    });
  });

  it('closeDeleteProjectDialog は自分だけを倒し、他のオーバーレイには触れない', () => {
    useAppStore.setState({
      deleteProjectDialog: PENDING,
      modal: { kind: 'edit_session', sessionId: 's9' },
      cleanupDialog: cleanup,
      projectSwitcherOpen: true,
    });

    useAppStore.getState().closeDeleteProjectDialog();

    const st = useAppStore.getState();
    expect(st.deleteProjectDialog).toBeNull();
    expect(st.modal).toEqual({ kind: 'edit_session', sessionId: 's9' });
    expect(st.cleanupDialog).toEqual(cleanup);
    expect(st.projectSwitcherOpen).toBe(true);
  });

  // 契約 §11.4.2 の「開いているモーダルを置き換える」を、§146.3 の読み
  // （「モーダル」= 画面を占有する overlay 一般）で満たす。3 フィールドを個別に見る
  // —— 1 つでも落とし損ねると overlay が 2 枚同時に開く。
  it('openDeleteProjectDialog は modal を落とす（契約 §11.4.2 / §146.3）', async () => {
    useAppStore.setState({ modal: { kind: 'create_session' } });

    await useAppStore.getState().openDeleteProjectDialog('p7');

    expect(useAppStore.getState().deleteProjectDialog?.projectId).toBe('p7');
    expect(useAppStore.getState().modal).toBeNull();
  });

  it('openDeleteProjectDialog は cleanupDialog を落とす（契約 §11.4.2 / §146.3）', async () => {
    useAppStore.setState({ cleanupDialog: cleanup });

    await useAppStore.getState().openDeleteProjectDialog('p7');

    expect(useAppStore.getState().deleteProjectDialog?.projectId).toBe('p7');
    expect(useAppStore.getState().cleanupDialog).toBeNull();
  });

  it('openDeleteProjectDialog は projectSwitcherOpen を落とす（契約 §11.4.2 / §146.3）', async () => {
    useAppStore.setState({ projectSwitcherOpen: true });

    await useAppStore.getState().openDeleteProjectDialog('p7');

    expect(useAppStore.getState().deleteProjectDialog?.projectId).toBe('p7');
    expect(useAppStore.getState().projectSwitcherOpen).toBe(false);
  });

  // 🔴 検査には向きがある。上の 3 本は「新 opener → 既存 3 つ」を測るだけで、
  // 「既存 opener → 新フィールド」は 1 文字も測らない（契約 §146.6 / §128）。
  it('setProjectSwitcherOpen(true) は deleteProjectDialog を落とす（逆向き。契約 §11.4.2）', () => {
    useAppStore.setState({ deleteProjectDialog: PENDING });

    useAppStore.getState().setProjectSwitcherOpen(true);

    expect(useAppStore.getState().projectSwitcherOpen).toBe(true);
    expect(useAppStore.getState().deleteProjectDialog).toBeNull();
  });

  it('setProjectSwitcherOpen(false) は deleteProjectDialog に触れない', () => {
    useAppStore.setState({ deleteProjectDialog: PENDING, projectSwitcherOpen: true });

    useAppStore.getState().setProjectSwitcherOpen(false);

    expect(useAppStore.getState().projectSwitcherOpen).toBe(false);
    expect(useAppStore.getState().deleteProjectDialog).toEqual(PENDING);
  });
});
