import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../ipc/commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../ipc/commands')>()),
  listSessions: vi.fn().mockResolvedValue([]),
}));

import { useAppStore } from '../store';
import { handleKeymapKeyDown } from './useKeymap';

const dispatch = (init: KeyboardEventInit) => {
  const event = new KeyboardEvent('keydown', { ...init, cancelable: true });
  window.dispatchEvent(event);
  return event;
};

// Cmd+T / Cmd+W の配線（契約 §29.8 Task 20）。
// 契約 §105.2.1: エラートーストを出すのは呼び出し側のハンドラである（ProjectBar.tsx の
// setActiveProject(...).catch(...) と同じ形）。ストアアクション自体は throw するだけで
// 良く、useKeymap.ts の switch の腕が .catch(setError) を持つことをここで固定する。
describe('useKeymap の Cmd+T / Cmd+W 配線', () => {
  beforeEach(() => {
    window.addEventListener('keydown', handleKeymapKeyDown);
    useAppStore.setState({ view: 'terminal', modal: null });
  });

  afterEach(() => {
    window.removeEventListener('keydown', handleKeymapKeyDown);
  });

  it('Cmd+T で preventDefault し、store.createScratchTerminal を呼ぶ', () => {
    const createScratchTerminal = vi.fn().mockResolvedValue(undefined);
    useAppStore.setState({ createScratchTerminal });

    const event = dispatch({ key: 't', metaKey: true });

    expect(event.defaultPrevented).toBe(true);
    expect(createScratchTerminal).toHaveBeenCalledTimes(1);
  });

  it('Cmd+W で preventDefault し、store.closeScratchTerminal を呼ぶ', () => {
    const closeScratchTerminal = vi.fn().mockResolvedValue(undefined);
    useAppStore.setState({ closeScratchTerminal });

    const event = dispatch({ key: 'w', metaKey: true });

    expect(event.defaultPrevented).toBe(true);
    expect(closeScratchTerminal).toHaveBeenCalledTimes(1);
  });

  it('createScratchTerminal が失敗したら setError を呼ぶ（契約 §105.2.1）', async () => {
    const error = { code: 'io' as const, message: 'boom-create' };
    const createScratchTerminal = vi.fn().mockRejectedValue(error);
    const setError = vi.fn();
    useAppStore.setState({ createScratchTerminal, setError });

    dispatch({ key: 't', metaKey: true });
    await vi.waitFor(() => expect(setError).toHaveBeenCalled());

    expect(setError).toHaveBeenCalledWith({ code: 'io', message: 'boom-create' });
  });

  it('closeScratchTerminal が失敗したら setError を呼ぶ（契約 §105.2.1）', async () => {
    const error = { code: 'io' as const, message: 'boom-close' };
    const closeScratchTerminal = vi.fn().mockRejectedValue(error);
    const setError = vi.fn();
    useAppStore.setState({ closeScratchTerminal, setError });

    dispatch({ key: 'w', metaKey: true });
    await vi.waitFor(() => expect(setError).toHaveBeenCalled());

    expect(setError).toHaveBeenCalledWith({ code: 'io', message: 'boom-close' });
  });
});
