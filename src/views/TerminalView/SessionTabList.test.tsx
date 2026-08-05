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
    runtimeStates: {},
    runtimeReasons: {},
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

describe('SessionTabList のバッジ（契約 §25.5 / §38.3）', () => {
  it('タブに RuntimeBadge を描く（runtimeStates に値があるときだけ）', () => {
    act(() => {
      root = createRoot(container);
      root.render(<SessionTabList />);
    });
    // 一度も起動していないセッションにはバッジを出さない（契約 §34.7）
    expect(container.querySelector('.runtime-badge')).toBeNull();

    act(() => {
      useAppStore
        .getState()
        .applyStateEvent({ session_id: 's1', runtime_state: 'running', reason: 'spawned' });
    });

    const badge = container.querySelector('.kamux-tab__meta .runtime-badge');
    expect(badge?.getAttribute('data-runtime-state')).toBe('running');
    // バッジは kamux-tab__cli より前に置く（components.md「セッションタブ」節）
    expect(badge?.nextElementSibling?.className).toBe('kamux-tab__cli');
  });

  it('実行状態が変わってもタブ列自体は再レンダリングしない', () => {
    // タブ列が runtimeStates を購読すると、セッション数だけ並ぶタブが全部描き直される。
    // Profiler は配下（バッジ）の再レンダリングでも発火してしまうため、描画中に読まれる
    // sessions[id].title の読み取り回数で SessionTabList 自身の再実行を数える。
    let titleReads = 0;
    const s1 = session('s1');
    const probe = Object.defineProperty({ ...s1 }, 'title', {
      get() {
        titleReads += 1;
        return s1.title;
      },
    });
    useAppStore.setState({ sessions: { s1: probe } });

    act(() => {
      root = createRoot(container);
      root.render(<SessionTabList />);
    });
    expect(titleReads).toBe(1);

    act(() => {
      useAppStore
        .getState()
        .applyStateEvent({ session_id: 's1', runtime_state: 'waiting_input', reason: 'hook_stop' });
    });

    expect(container.querySelector('.runtime-badge')?.getAttribute('data-runtime-state')).toBe(
      'waiting_input',
    );
    expect(titleReads).toBe(1);
  });
});
