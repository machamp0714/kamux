import { beforeEach, describe, expect, it, vi } from 'vitest';
import { renderHook } from '@testing-library/react';

vi.mock('../ipc/commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../ipc/commands')>()),
  listSessions: vi.fn().mockResolvedValue([]),
}));

import { useAppStore } from '../store';
import { useKeymap } from './useKeymap';

describe('Cmd+P', () => {
  beforeEach(() => {
    useAppStore.setState({ projectSwitcherOpen: false, modal: null, cleanupDialog: null });
  });

  it('Cmd+P でプロジェクトスイッチャーが開く', () => {
    renderHook(() => useKeymap());

    const ev = new KeyboardEvent('keydown', { key: 'p', metaKey: true, cancelable: true });
    window.dispatchEvent(ev);

    expect(useAppStore.getState().projectSwitcherOpen).toBe(true);
    expect(ev.defaultPrevented).toBe(true);
  });

  it('metaKey なしの p では開かない（ターミナル入力を奪わない）', () => {
    renderHook(() => useKeymap());

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'p', cancelable: true }));

    expect(useAppStore.getState().projectSwitcherOpen).toBe(false);
  });

  it('開いているときに Cmd+P を押すと閉じる（トグル）', () => {
    useAppStore.setState({ projectSwitcherOpen: true });
    renderHook(() => useKeymap());

    window.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'p', metaKey: true, cancelable: true }),
    );

    expect(useAppStore.getState().projectSwitcherOpen).toBe(false);
  });

  // 契約 §97.2 規則 S: Shift は排他にしない。英字キーは大文字の分岐を持ち、
  // Shift 併用でも同じアクションを返す。resolveKeymap の `|| e.key === 'P'` の観測点。
  it('Cmd+Shift+P（key = "P"）でも開く（契約 §97.2 規則 S）', () => {
    renderHook(() => useKeymap());

    window.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'P', metaKey: true, shiftKey: true, cancelable: true }),
    );

    expect(useAppStore.getState().projectSwitcherOpen).toBe(true);
  });

  // 契約 §97.2 規則 C の 7 キー（Cmd+J / Cmd+K / Cmd+D / Cmd+[ / Cmd+] / Cmd+T / Cmd+W）に
  // Cmd+P は入っていない。表の `Cmd+N` / `Cmd+P` 行は Ctrl 併用「発火する」。
  it('Ctrl 併用の Cmd+P でも開く（契約 §97.2 規則 C の集合外）', () => {
    renderHook(() => useKeymap());

    window.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'p', metaKey: true, ctrlKey: true, cancelable: true }),
    );

    expect(useAppStore.getState().projectSwitcherOpen).toBe(true);
  });

  // 契約 §11.4.2 の Cmd+P 行の view 条件は「無し」。terminal / editor でも発火する。
  it('terminal 画面でも editor 画面でも Cmd+P で開く（契約 §11.4.2: view 条件は無し）', () => {
    for (const view of ['terminal', 'editor'] as const) {
      useAppStore.setState({ projectSwitcherOpen: false, view });
      renderHook(() => useKeymap());

      window.dispatchEvent(
        new KeyboardEvent('keydown', { key: 'p', metaKey: true, cancelable: true }),
      );

      expect(useAppStore.getState().projectSwitcherOpen).toBe(true);
    }
    useAppStore.setState({ view: 'kanban' });
  });
});
