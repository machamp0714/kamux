import { act, render } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SessionStatePayload } from '../types/model';

const handlers = new Map<string, (p: SessionStatePayload) => void>();
const unlistened: string[] = [];

vi.mock('../ipc/events', () => ({
  listenSessionState: vi.fn(async (id: string, cb: (p: SessionStatePayload) => void) => {
    handlers.set(id, cb);
    return () => {
      unlistened.push(id);
      handlers.delete(id);
    };
  }),
}));

import { listenSessionState } from '../ipc/events';
import { useRuntimeStateEvents } from './useRuntimeStateEvents';
import { useAppStore } from '../store';

function Probe({ ids }: { ids: string[] }) {
  useRuntimeStateEvents(ids);
  return null;
}

const flush = () =>
  act(async () => {
    await Promise.resolve();
  });

describe('useRuntimeStateEvents', () => {
  beforeEach(() => {
    handlers.clear();
    unlistened.length = 0;
    vi.mocked(listenSessionState).mockClear();
    useAppStore.setState({ runtimeStates: {}, runtimeReasons: {} });
  });

  it('subscribes once per session id', async () => {
    render(<Probe ids={['a', 'b']} />);
    await flush();
    expect(
      vi
        .mocked(listenSessionState)
        .mock.calls.map((c) => c[0])
        .sort(),
    ).toEqual(['a', 'b']);
  });

  it('routes payloads into the store', async () => {
    render(<Probe ids={['a']} />);
    await flush();
    act(() => {
      handlers.get('a')?.({ session_id: 'a', runtime_state: 'running', reason: 'spawned' });
    });
    expect(useAppStore.getState().runtimeStates.a).toBe('running');
  });

  it('only churns the delta when the id list changes', async () => {
    const { rerender } = render(<Probe ids={['a', 'b']} />);
    await flush();
    vi.mocked(listenSessionState).mockClear();

    rerender(<Probe ids={['a', 'b', 'c']} />);
    await flush();

    expect(vi.mocked(listenSessionState).mock.calls.map((c) => c[0])).toEqual(['c']);
    expect(unlistened).toEqual([]);
  });

  it('ignores reordering of the same ids', async () => {
    const { rerender } = render(<Probe ids={['a', 'b']} />);
    await flush();
    vi.mocked(listenSessionState).mockClear();

    rerender(<Probe ids={['b', 'a']} />);
    await flush();

    expect(vi.mocked(listenSessionState)).not.toHaveBeenCalled();
    expect(unlistened).toEqual([]);
  });

  it('unlistens removed ids', async () => {
    const { rerender } = render(<Probe ids={['a', 'b']} />);
    await flush();
    rerender(<Probe ids={['a']} />);
    await flush();
    expect(unlistened).toEqual(['b']);
  });

  it('unlistens everything on unmount', async () => {
    const { unmount } = render(<Probe ids={['a', 'b']} />);
    await flush();
    unmount();
    await flush();
    expect(unlistened.sort()).toEqual(['a', 'b']);
  });

  it('does not leak a listener when it is removed before listen() resolves', async () => {
    // listenSessionState('a', ...) の Promise がまだ解決していないうちに
    // 'a' を id リストから外す。'pending' の経路を通らないと、解決後に
    // unlisten されないリスナーが残ってしまう。
    const { rerender } = render(<Probe ids={['a']} />);
    rerender(<Probe ids={[]} />);
    await flush();
    expect(unlistened).toEqual(['a']);
  });
});
