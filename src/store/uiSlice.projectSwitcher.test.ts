import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../ipc/commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../ipc/commands')>()),
  listSessions: vi.fn().mockResolvedValue([]),
}));

import { useAppStore } from './index';
import type { CleanupDialogState } from './cleanup';

const dialog: CleanupDialogState = {
  sessionId: 's1',
  status: null,
  error: null,
  busy: false,
};

describe('setProjectSwitcherOpen', () => {
  beforeEach(() => {
    useAppStore.setState({ projectSwitcherOpen: false, modal: null, cleanupDialog: null });
  });

  it('既定では閉じている', () => {
    expect(useAppStore.getState().projectSwitcherOpen).toBe(false);
  });

  // 契約 §11.4.2 の Cmd+P 行は「発火する（開いているモーダルを置き換える）」。
  // modal と cleanupDialog は独立フィールド（uiSlice.ts の 2 つの set）なので、
  // 素直に projectSwitcherOpen だけを立てるとオーバーレイが 2 枚同時に開く。
  it('開くときは開いている modal と cleanupDialog を閉じる（置き換える。契約 §11.4.2）', () => {
    useAppStore.setState({ modal: { kind: 'create_session' }, cleanupDialog: dialog });

    useAppStore.getState().setProjectSwitcherOpen(true);

    expect(useAppStore.getState().projectSwitcherOpen).toBe(true);
    expect(useAppStore.getState().modal).toBeNull();
    expect(useAppStore.getState().cleanupDialog).toBeNull();
  });

  it('閉じるときは projectSwitcherOpen だけを倒し、他のオーバーレイには触れない', () => {
    useAppStore.setState({
      projectSwitcherOpen: true,
      modal: { kind: 'edit_session', sessionId: 's9' },
      cleanupDialog: dialog,
    });

    useAppStore.getState().setProjectSwitcherOpen(false);

    expect(useAppStore.getState().projectSwitcherOpen).toBe(false);
    expect(useAppStore.getState().modal).toEqual({ kind: 'edit_session', sessionId: 's9' });
    expect(useAppStore.getState().cleanupDialog).toEqual(dialog);
  });
});
