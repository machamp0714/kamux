import { beforeEach, describe, expect, it, vi } from 'vitest';

const worktreeStatus = vi.fn();
const cleanupWorktree = vi.fn();
const listSessions = vi.fn();
vi.mock('../ipc/commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../ipc/commands')>()),
  worktreeStatus: (...a: unknown[]) => worktreeStatus(...a),
  cleanupWorktree: (...a: unknown[]) => cleanupWorktree(...a),
  listSessions: (...a: unknown[]) => listSessions(...a),
}));

import { useAppStore } from './index';

describe('掃除ダイアログ', () => {
  beforeEach(() => {
    worktreeStatus.mockReset();
    cleanupWorktree.mockReset();
    listSessions.mockReset().mockResolvedValue([]);
    useAppStore.setState({ cleanupDialog: null, activeProjectId: 'p1' });
  });

  it('開くと worktree_status を取りに行き、結果を保持する', async () => {
    worktreeStatus.mockResolvedValue({ dirty: true, entries: ['?? new.txt'] });

    await useAppStore.getState().openCleanupDialog('s1');

    expect(worktreeStatus).toHaveBeenCalledWith('s1');
    expect(useAppStore.getState().cleanupDialog).toEqual({
      sessionId: 's1',
      status: { dirty: true, entries: ['?? new.txt'] },
      error: null,
      busy: false,
    });
  });

  it('status 取得に失敗したら error に生の message を入れる', async () => {
    worktreeStatus.mockRejectedValue({ code: 'git', message: 'fatal: not a git repository\n' });

    await useAppStore.getState().openCleanupDialog('s1');

    expect(useAppStore.getState().cleanupDialog?.error).toBe('fatal: not a git repository\n');
    expect(useAppStore.getState().cleanupDialog?.status).toBeNull();
  });

  it('確定すると cleanup_worktree を呼び、成功でダイアログを閉じて再ロードする', async () => {
    worktreeStatus.mockResolvedValue({ dirty: false, entries: [] });
    cleanupWorktree.mockResolvedValue(undefined);
    await useAppStore.getState().openCleanupDialog('s1');

    await useAppStore.getState().confirmCleanup(false);

    expect(cleanupWorktree).toHaveBeenCalledWith('s1', false);
    expect(useAppStore.getState().cleanupDialog).toBeNull();
    expect(listSessions).toHaveBeenCalledWith('p1', true);
  });

  it('削除に失敗したらダイアログは開いたまま error を出す', async () => {
    worktreeStatus.mockResolvedValue({ dirty: true, entries: ['?? new.txt'] });
    cleanupWorktree.mockRejectedValue({
      code: 'git',
      message: "fatal: '/x' contains modified or untracked files, use --force to delete it\n",
    });
    await useAppStore.getState().openCleanupDialog('s1');

    await useAppStore.getState().confirmCleanup(false);

    const d = useAppStore.getState().cleanupDialog;
    expect(d).not.toBeNull();
    expect(d?.error).toContain('--force');
    expect(d?.busy).toBe(false);
  });

  it('閉じると状態が消える', async () => {
    worktreeStatus.mockResolvedValue({ dirty: false, entries: [] });
    await useAppStore.getState().openCleanupDialog('s1');

    useAppStore.getState().closeCleanupDialog();

    expect(useAppStore.getState().cleanupDialog).toBeNull();
  });
});
