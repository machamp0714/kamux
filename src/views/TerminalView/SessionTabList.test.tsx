import { act, Profiler, type ProfilerOnRenderCallback } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

// React 18 の act() は既定でこのフラグを見て、テスト環境かどうかを判定する
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

import { useAppStore } from '../../store';
import type { KanbanStatus, Session } from '../../types/model';
import { SessionTabList } from './SessionTabList';

function session(id: string): Session {
  return {
    id,
    project_id: 'p1',
    title: id,
    description: '',
    kanban_status: 'in_progress',
    sort_order: 1,
    mode: 'in_place',
    branch: null,
    worktree_path: null,
    cli_kind: 'shell',
    cli_command: null,
    claude_session_id: null,
    last_runtime_state: 'idle',
    last_runtime_error: null,
    first_started_at: null,
    archived_at: null,
    created_at: 0,
    updated_at: 0,
  };
}

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  const emptyOrder: Record<KanbanStatus, string[]> = {
    backlog: [],
    in_progress: ['s1'],
    review: [],
    done: [],
  };
  useAppStore.setState({
    sessions: { s1: session('s1') },
    sessionOrder: emptyOrder,
    paneAssignment: ['s1', null],
    activePane: 0,
  });

  container = document.createElement('div');
  document.body.appendChild(container);
});

afterEach(() => {
  act(() => {
    root.unmount();
  });
  container.remove();
});

describe('SessionTabList（必達 5: useShallow）', () => {
  it('セッション一覧と無関係なストア更新では再レンダリングしない', () => {
    let renderCount = 0;
    const onRender: ProfilerOnRenderCallback = () => {
      renderCount += 1;
    };

    act(() => {
      root = createRoot(container);
      root.render(
        <Profiler id="tabs" onRender={onRender}>
          <SessionTabList />
        </Profiler>,
      );
    });
    expect(renderCount).toBe(1);

    // sessions / sessionOrder / paneAssignment / activePane のどれにも触れない更新。
    // selectTerminalTabs は毎回新しい配列を返す純関数なので、useShallow が無いと
    // この無関係な set() だけで SessionTabList が再レンダリングされる
    act(() => {
      useAppStore.getState().setError({ code: 'io', message: 'unrelated' });
    });

    expect(renderCount).toBe(1);
  });
});
