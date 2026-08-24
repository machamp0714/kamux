import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../ipc/commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../ipc/commands')>()),
  listSessions: vi.fn().mockResolvedValue([]),
}));

import { useAppStore } from './index';
import type { CleanupDialogState } from './cleanup';

const cleanup: CleanupDialogState = {
  sessionId: 's1',
  status: null,
  error: null,
  busy: false,
};

describe('deleteProjectDialog', () => {
  beforeEach(() => {
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

  it('openDeleteProjectDialog は削除対象のプロジェクトを持って開く', () => {
    useAppStore.getState().openDeleteProjectDialog('p7');

    // 取り違えたら別物になる具体値で観測する（開閉フラグではなく対象 id を持つこと）。
    expect(useAppStore.getState().deleteProjectDialog).toEqual({ projectId: 'p7' });
  });

  it('closeDeleteProjectDialog は自分だけを倒し、他のオーバーレイには触れない', () => {
    useAppStore.setState({
      deleteProjectDialog: { projectId: 'p7' },
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
  it('openDeleteProjectDialog は modal を落とす（契約 §11.4.2 / §146.3）', () => {
    useAppStore.setState({ modal: { kind: 'create_session' } });

    useAppStore.getState().openDeleteProjectDialog('p7');

    expect(useAppStore.getState().deleteProjectDialog).toEqual({ projectId: 'p7' });
    expect(useAppStore.getState().modal).toBeNull();
  });

  it('openDeleteProjectDialog は cleanupDialog を落とす（契約 §11.4.2 / §146.3）', () => {
    useAppStore.setState({ cleanupDialog: cleanup });

    useAppStore.getState().openDeleteProjectDialog('p7');

    expect(useAppStore.getState().deleteProjectDialog).toEqual({ projectId: 'p7' });
    expect(useAppStore.getState().cleanupDialog).toBeNull();
  });

  it('openDeleteProjectDialog は projectSwitcherOpen を落とす（契約 §11.4.2 / §146.3）', () => {
    useAppStore.setState({ projectSwitcherOpen: true });

    useAppStore.getState().openDeleteProjectDialog('p7');

    expect(useAppStore.getState().deleteProjectDialog).toEqual({ projectId: 'p7' });
    expect(useAppStore.getState().projectSwitcherOpen).toBe(false);
  });

  // 🔴 検査には向きがある。上の 3 本は「新 opener → 既存 3 つ」を測るだけで、
  // 「既存 opener → 新フィールド」は 1 文字も測らない（契約 §146.6 / §128）。
  it('setProjectSwitcherOpen(true) は deleteProjectDialog を落とす（逆向き。契約 §11.4.2）', () => {
    useAppStore.setState({ deleteProjectDialog: { projectId: 'p7' } });

    useAppStore.getState().setProjectSwitcherOpen(true);

    expect(useAppStore.getState().projectSwitcherOpen).toBe(true);
    expect(useAppStore.getState().deleteProjectDialog).toBeNull();
  });

  it('setProjectSwitcherOpen(false) は deleteProjectDialog に触れない', () => {
    useAppStore.setState({ deleteProjectDialog: { projectId: 'p7' }, projectSwitcherOpen: true });

    useAppStore.getState().setProjectSwitcherOpen(false);

    expect(useAppStore.getState().projectSwitcherOpen).toBe(false);
    expect(useAppStore.getState().deleteProjectDialog).toEqual({ projectId: 'p7' });
  });
});
