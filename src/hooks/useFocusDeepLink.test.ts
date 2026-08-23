import { describe, expect, it, vi, beforeEach } from 'vitest';
import { renderHook, waitFor } from '@testing-library/react';

const listeners = new Map<string, (p: unknown) => void>();
const unlisten = vi.fn();

vi.mock('../ipc/events', () => ({
  listenFocus: (sessionId: string, cb: (p: unknown) => void) => {
    listeners.set(sessionId, cb);
    return Promise.resolve(unlisten);
  },
}));

const focusSession = vi.fn();
let sessions: Record<string, { id: string }> = { s1: { id: 's1' }, s2: { id: 's2' } };
vi.mock('../store', () => ({
  useAppStore: (selector: (s: unknown) => unknown) => selector({ sessions, focusSession }),
}));

import { useFocusDeepLink } from './useFocusDeepLink';

describe('useFocusDeepLink', () => {
  beforeEach(() => {
    listeners.clear();
    focusSession.mockClear();
    unlisten.mockClear();
    sessions = { s1: { id: 's1' }, s2: { id: 's2' } };
  });

  it('ストア上の全セッションを購読する', async () => {
    renderHook(() => useFocusDeepLink());
    await waitFor(() => expect(listeners.size).toBe(2));
    expect([...listeners.keys()].sort()).toEqual(['s1', 's2']);
  });

  it('イベントを受けたらターミナル画面で該当セッションにフォーカスする', async () => {
    renderHook(() => useFocusDeepLink());
    await waitFor(() => expect(listeners.size).toBe(2));

    listeners.get('s2')?.({ session_id: 's9', surface_kind: 'agent' });

    expect(focusSession).toHaveBeenCalledWith('s9', 'terminal');
  });

  it('surface_kind が editor のときはエディタ画面にフォーカスする（Ruling AF: routing の退行を検出する）', async () => {
    renderHook(() => useFocusDeepLink());
    await waitFor(() => expect(listeners.size).toBe(2));

    listeners.get('s2')?.({ session_id: 's2', surface_kind: 'editor' });

    expect(focusSession).toHaveBeenCalledWith('s2', 'editor');
  });

  it('アンマウント時に購読を解除する', async () => {
    const { unmount } = renderHook(() => useFocusDeepLink());
    await waitFor(() => expect(listeners.size).toBe(2));
    unmount();
    await waitFor(() => expect(unlisten).toHaveBeenCalledTimes(2));
  });

  it('セッションが増えたら新しい ID を追加購読する（依存配列が sessionIdsKey を含むことを守る）', async () => {
    sessions = { s1: { id: 's1' } };
    const { rerender } = renderHook(() => useFocusDeepLink());
    await waitFor(() => expect(listeners.size).toBe(1));

    sessions = { s1: { id: 's1' }, s2: { id: 's2' } };
    rerender();
    await waitFor(() => expect(listeners.size).toBe(2));
    expect([...listeners.keys()].sort()).toEqual(['s1', 's2']);
  });
});
