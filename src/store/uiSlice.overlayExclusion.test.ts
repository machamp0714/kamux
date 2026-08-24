import { beforeEach, describe, expect, it, vi } from 'vitest';

const worktreeStatus = vi.fn();
vi.mock('../ipc/commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../ipc/commands')>()),
  worktreeStatus: (...a: unknown[]) => worktreeStatus(...a),
}));

import { useAppStore } from './index';
import type { CleanupDialogState } from './cleanup';
import type { WorktreeStatus } from '../types/model';

const cleanupDialog: CleanupDialogState = {
  sessionId: 's1',
  status: null,
  error: null,
  busy: false,
};

const PENDING_DELETE = { projectId: 'p7', sessions: null, error: null };

// 契約 §11.4.2 / §146.3 / §146.6 —— `modal` と `cleanupDialog` は「開くときに他を落とさない」の
// 2 行として非適合が確定していた（12-C の射程）。
describe('overlay 相互排他（openModal / openCleanupDialog）', () => {
  beforeEach(() => {
    worktreeStatus.mockReset().mockResolvedValue({ dirty: false, entries: [] });
    useAppStore.setState({
      modal: null,
      cleanupDialog: null,
      projectSwitcherOpen: false,
      deleteProjectDialog: null,
    });
  });

  it('openModal は cleanupDialog を落とす', () => {
    useAppStore.setState({ cleanupDialog });

    useAppStore.getState().openModal({ kind: 'create_session' });

    expect(useAppStore.getState().cleanupDialog).toBeNull();
  });

  it('openModal は projectSwitcherOpen を落とす', () => {
    useAppStore.setState({ projectSwitcherOpen: true });

    useAppStore.getState().openModal({ kind: 'create_session' });

    expect(useAppStore.getState().projectSwitcherOpen).toBe(false);
  });

  it('openModal は deleteProjectDialog を落とす', () => {
    useAppStore.setState({ deleteProjectDialog: PENDING_DELETE });

    useAppStore.getState().openModal({ kind: 'create_session' });

    expect(useAppStore.getState().deleteProjectDialog).toBeNull();
  });

  it('openModal は view を kanban へ立て続ける（契約 §11。ターミナル画面からも Cmd+N が効く副作用）', () => {
    useAppStore.setState({ view: 'terminal' });

    useAppStore.getState().openModal({ kind: 'create_session' });

    expect(useAppStore.getState().view).toBe('kanban');
  });

  it('openCleanupDialog は modal を落とす', () => {
    useAppStore.setState({ modal: { kind: 'create_session' } });

    useAppStore.getState().openCleanupDialog('s1');

    expect(useAppStore.getState().modal).toBeNull();
  });

  it('openCleanupDialog は projectSwitcherOpen を落とす', () => {
    useAppStore.setState({ projectSwitcherOpen: true });

    useAppStore.getState().openCleanupDialog('s1');

    expect(useAppStore.getState().projectSwitcherOpen).toBe(false);
  });

  it('openCleanupDialog は deleteProjectDialog を落とす', () => {
    useAppStore.setState({ deleteProjectDialog: PENDING_DELETE });

    useAppStore.getState().openCleanupDialog('s1');

    expect(useAppStore.getState().deleteProjectDialog).toBeNull();
  });

  // 裁定 101 が防いだ欠陥の逆向きの観測点。`await worktreeStatus` 後の 2 つの `set`
  // （成功側・catch 側）はガード（`st.cleanupDialog?.sessionId === sessionId`）付きの
  // 遅延応答であり、往復中に cleanupDialog が閉じられていたら modal には触れてはいけない。
  // ガードを外して `modal: null` を無条件に足す変異（群3）を打つとここが赤くなる。
  it('openCleanupDialog の遅延応答は、往復中に対象がずれたら modal に触れない', async () => {
    let resolveStatus: (value: WorktreeStatus) => void = () => {};
    worktreeStatus.mockImplementation(
      () =>
        new Promise<WorktreeStatus>((resolve) => {
          resolveStatus = resolve;
        }),
    );

    const pending = useAppStore.getState().openCleanupDialog('s1');
    // 往復中に cleanupDialog が閉じられ（対象ガードが外れ）、modal が開かれる。
    useAppStore.setState({ cleanupDialog: null, modal: { kind: 'create_session' } });

    resolveStatus({ dirty: true, entries: ['?? new.txt'] });
    await pending;

    expect(useAppStore.getState().modal).toEqual({ kind: 'create_session' });
  });

  // 上のテストの鏡像（catch 側）。`await worktreeStatus` が reject する経路にも同じ
  // ガード（`st.cleanupDialog?.sessionId === sessionId`）があり、往復中に cleanupDialog が
  // 閉じられていたら modal には触れてはいけない。catch 側のガードの外側で無条件に
  // `modal: null` を足す変異（成功側と鏡像の欠陥）を打つとここが赤くなる。
  it('openCleanupDialog の遅延応答（catch 側）は、往復中に対象がずれたら modal に触れない', async () => {
    let rejectStatus: (e: { code: string; message: string }) => void = () => {};
    worktreeStatus.mockImplementation(
      () =>
        new Promise<WorktreeStatus>((_resolve, reject) => {
          rejectStatus = reject;
        }),
    );

    const pending = useAppStore.getState().openCleanupDialog('s1');
    // 往復中に cleanupDialog が閉じられ（対象ガードが外れ）、modal が開かれる。
    useAppStore.setState({ cleanupDialog: null, modal: { kind: 'create_session' } });

    rejectStatus({ code: 'git', message: 'stale error' });
    await pending;

    expect(useAppStore.getState().modal).toEqual({ kind: 'create_session' });
  });
});
