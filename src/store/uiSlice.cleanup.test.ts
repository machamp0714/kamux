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

  it('削除に失敗したらダイアログは開いたまま error を出す。再試行の開始時点で前回の error を消す', async () => {
    worktreeStatus.mockResolvedValue({ dirty: true, entries: ['?? new.txt'] });
    cleanupWorktree.mockRejectedValueOnce({
      code: 'git',
      message: "fatal: '/x' contains modified or untracked files, use --force to delete it\n",
    });
    await useAppStore.getState().openCleanupDialog('s1');

    await useAppStore.getState().confirmCleanup(false);

    const d = useAppStore.getState().cleanupDialog;
    expect(d).not.toBeNull();
    expect(d?.error).toContain('--force');
    expect(d?.busy).toBe(false);

    // 再試行（--force 付き）。cleanupWorktree の解決を手元で止め、開始直後（await 前）の
    // 状態を覗いて、前回の error が再試行開始と同時に消えていることを確認する。
    let resolveRetry: () => void = () => {};
    const retryResponse = new Promise<void>((resolve) => {
      resolveRetry = resolve;
    });
    cleanupWorktree.mockImplementationOnce(() => retryResponse);

    const retryPromise = useAppStore.getState().confirmCleanup(true);
    expect(useAppStore.getState().cleanupDialog?.busy).toBe(true);
    expect(useAppStore.getState().cleanupDialog?.error).toBeNull();

    resolveRetry();
    await retryPromise;
    expect(useAppStore.getState().cleanupDialog).toBeNull();
  });

  it('閉じると状態が消える', async () => {
    worktreeStatus.mockResolvedValue({ dirty: false, entries: [] });
    await useAppStore.getState().openCleanupDialog('s1');

    useAppStore.getState().closeCleanupDialog();

    expect(useAppStore.getState().cleanupDialog).toBeNull();
  });

  it('先に開いた s1 の遅延応答が、後から開いた s2 のダイアログを上書きしない', async () => {
    let resolveS1: (v: { dirty: boolean; entries: string[] }) => void = () => {};
    const s1Response = new Promise<{ dirty: boolean; entries: string[] }>((resolve) => {
      resolveS1 = resolve;
    });
    worktreeStatus.mockImplementation((sessionId: unknown) =>
      sessionId === 's1' ? s1Response : Promise.resolve({ dirty: false, entries: [] }),
    );

    const p1 = useAppStore.getState().openCleanupDialog('s1');
    const p2 = useAppStore.getState().openCleanupDialog('s2');
    await p2;

    // s2 が先に解決した後で、s1 の遅れた応答が届く。
    resolveS1({ dirty: true, entries: ['?? stale.txt'] });
    await p1;

    expect(useAppStore.getState().cleanupDialog).toEqual({
      sessionId: 's2',
      status: { dirty: false, entries: [] },
      error: null,
      busy: false,
    });
  });

  it('先に開いた s1 の遅延エラーが、後から開いた s2 のダイアログを汚さない', async () => {
    let rejectS1: (e: { code: string; message: string }) => void = () => {};
    const s1Response = new Promise<{ dirty: boolean; entries: string[] }>((_resolve, reject) => {
      rejectS1 = reject;
    });
    worktreeStatus.mockImplementation((sessionId: unknown) =>
      sessionId === 's1' ? s1Response : Promise.resolve({ dirty: false, entries: [] }),
    );

    const p1 = useAppStore.getState().openCleanupDialog('s1');
    const p2 = useAppStore.getState().openCleanupDialog('s2');
    await p2;

    // s2 が先に解決した後で、s1 の遅れたエラーが届く。
    rejectS1({ code: 'git', message: 'stale error' });
    await p1;

    expect(useAppStore.getState().cleanupDialog).toEqual({
      sessionId: 's2',
      status: { dirty: false, entries: [] },
      error: null,
      busy: false,
    });
  });
});
