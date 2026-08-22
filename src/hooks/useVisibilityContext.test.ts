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

  interface RerenderCase {
    name: string;
    initial: MockState;
    expectedInitial: [string, string[]];
    mutate: (s: MockState) => MockState;
    expectedAfter: [string, string[]];
  }

  const rerenderCases: RerenderCase[] = [
    {
      name: 'view',
      initial: {
        view: 'terminal',
        layout: 'single',
        focusedSessionId: 's1',
        paneAssignment: [null, null],
      },
      expectedInitial: ['terminal', ['s1']],
      mutate: (s) => ({ ...s, view: 'kanban' }),
      expectedAfter: ['kanban', []],
    },
    {
      name: 'layout',
      initial: {
        view: 'terminal',
        layout: 'single',
        focusedSessionId: 's1',
        paneAssignment: [null, null],
      },
      expectedInitial: ['terminal', ['s1']],
      mutate: (s) => ({ ...s, layout: 'split2' }),
      expectedAfter: ['terminal', []],
    },
    {
      name: 'focusedSessionId',
      initial: {
        view: 'terminal',
        layout: 'single',
        focusedSessionId: 's1',
        paneAssignment: [null, null],
      },
      expectedInitial: ['terminal', ['s1']],
      mutate: (s) => ({ ...s, focusedSessionId: 's2' }),
      expectedAfter: ['terminal', ['s2']],
    },
    {
      name: 'paneAssignment',
      initial: {
        view: 'terminal',
        layout: 'split2',
        focusedSessionId: null,
        paneAssignment: ['s1', null],
      },
      expectedInitial: ['terminal', ['s1']],
      mutate: (s) => ({ ...s, paneAssignment: ['s1', 's2'] }),
      expectedAfter: ['terminal', ['s1', 's2']],
    },
  ];

  it.each(rerenderCases)(
    '$name だけを変えて rerender すると新しい値で push し直し、無関係な rerender では push しない',
    async ({ initial, expectedInitial, mutate, expectedAfter }) => {
      state = initial;
      const { rerender } = renderHook(() => useVisibilityContext());
      await waitFor(() => expect(setVisibilityContext).toHaveBeenCalledWith(...expectedInitial));
      expect(setVisibilityContext).toHaveBeenCalledTimes(1);
      setVisibilityContext.mockClear();

      // 依存配列に何も変化が無い rerender。依存配列が正しく効いていれば
      // ここでは push が起きないはず（依存配列ごと削除する退行の検知点）。
      rerender();
      await new Promise((resolve) => setTimeout(resolve, 0));
      expect(setVisibilityContext).not.toHaveBeenCalled();

      state = mutate(state);
      rerender();
      await waitFor(() => expect(setVisibilityContext).toHaveBeenCalledWith(...expectedAfter));
      expect(setVisibilityContext).toHaveBeenCalledTimes(1);
    },
  );
});
