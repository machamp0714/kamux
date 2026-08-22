import { describe, expect, it, vi, beforeEach } from 'vitest';
import { renderHook, waitFor } from '@testing-library/react';

const setVisibilityContext = vi.fn();
vi.mock('../ipc/commands', () => ({
  setVisibilityContext: (view: unknown, ids: unknown) => setVisibilityContext(view, ids),
}));

interface MockState {
  view: 'kanban' | 'terminal' | 'editor';
  layout: 'single' | 'split2' | 'split2-v';
  focusedSessionId: string | null;
  paneAssignment: [string | null, string | null];
}

let state: MockState;

vi.mock('../store', () => ({
  useAppStore: (selector: (s: MockState) => unknown) => selector(state),
}));

import { useVisibilityContext } from './useVisibilityContext';

describe('useVisibilityContext', () => {
  beforeEach(() => {
    setVisibilityContext.mockClear();
  });

  it('terminal / single では focusedSessionId 1件を push する', async () => {
    state = {
      view: 'terminal',
      layout: 'single',
      focusedSessionId: 's1',
      paneAssignment: [null, null],
    };
    renderHook(() => useVisibilityContext());
    await waitFor(() => expect(setVisibilityContext).toHaveBeenCalledWith('terminal', ['s1']));
  });

  it('kanban では view は押すが id は空配列になる', async () => {
    state = {
      view: 'kanban',
      layout: 'single',
      focusedSessionId: 's1',
      paneAssignment: [null, null],
    };
    renderHook(() => useVisibilityContext());
    await waitFor(() => expect(setVisibilityContext).toHaveBeenCalledWith('kanban', []));
  });

  it('split2 では両ペインの id が渡る', async () => {
    state = {
      view: 'terminal',
      layout: 'split2',
      focusedSessionId: null,
      paneAssignment: ['s1', 's2'],
    };
    renderHook(() => useVisibilityContext());
    await waitFor(() =>
      expect(setVisibilityContext).toHaveBeenCalledWith('terminal', ['s1', 's2']),
    );
  });

  it('view が変わるたびに push し直す（依存配列が全フィールドを含むことを守る）', async () => {
    state = {
      view: 'terminal',
      layout: 'single',
      focusedSessionId: 's1',
      paneAssignment: [null, null],
    };
    const { rerender } = renderHook(() => useVisibilityContext());
    await waitFor(() => expect(setVisibilityContext).toHaveBeenCalledWith('terminal', ['s1']));

    state = { ...state, view: 'kanban' };
    rerender();
    await waitFor(() => expect(setVisibilityContext).toHaveBeenCalledWith('kanban', []));
    expect(setVisibilityContext).toHaveBeenCalledTimes(2);
  });
});
