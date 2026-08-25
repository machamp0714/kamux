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
    heuristics_enabled: true,
    silence_timeout_secs: 30,
    is_scratch: false,
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

describe('SessionTabList のペインバッジ（契約 §28.3）', () => {
  function setTabs(): void {
    useAppStore.setState({
      sessions: { s1: session('s1'), s2: session('s2'), s3: session('s3') },
      sessionOrder: { backlog: [], in_progress: ['s1', 's2', 's3'], review: [], done: [] },
    });
  }

  function badgeOf(id: string): string | null {
    const tab = container.querySelector(`[data-session-id="${id}"]`);
    return tab?.querySelector('.kamux-tab__pane-badge')?.textContent ?? null;
  }

  function renderTabs(): void {
    act(() => {
      root = createRoot(container);
      root.render(<SessionTabList />);
    });
  }

  it('split2 では左ペインに L、右ペインに R を出す', () => {
    setTabs();
    useAppStore.setState({ layout: 'split2', paneAssignment: ['s1', 's2'], activePane: 0 });
    renderTabs();

    expect(badgeOf('s1')).toBe('L');
    expect(badgeOf('s2')).toBe('R');
    // どちらのペインにも出ていないセッションにはバッジを出さない
    expect(badgeOf('s3')).toBeNull();
  });

  it('split2-v では上ペインに U、下ペインに D を出す（クラス名は変えない）', () => {
    setTabs();
    useAppStore.setState({ layout: 'split2-v', paneAssignment: ['s1', 's2'], activePane: 1 });
    renderTabs();

    expect(badgeOf('s1')).toBe('U');
    expect(badgeOf('s2')).toBe('D');
    expect(container.querySelectorAll('.kamux-tab__pane-badge')).toHaveLength(2);
  });

  it('single ではバッジを出さない（ペインの概念を見せない）', () => {
    setTabs();
    useAppStore.setState({ layout: 'single', paneAssignment: ['s1', 's2'], activePane: 0 });
    renderTabs();

    expect(container.querySelector('.kamux-tab__pane-badge')).toBeNull();
  });

  it('レイアウトを切り替えるとバッジの向きが追従する', () => {
    setTabs();
    useAppStore.setState({ layout: 'split2', paneAssignment: ['s1', 's2'], activePane: 0 });
    renderTabs();
    expect(badgeOf('s1')).toBe('L');

    act(() => {
      useAppStore.getState().setLayout('split2-v');
    });

    expect(badgeOf('s1')).toBe('U');
    expect(badgeOf('s2')).toBe('D');
  });
});

describe('SessionTabList の 2 グループ表示（契約 §29.7）', () => {
  function renderTabs(): void {
    act(() => {
      root = createRoot(container);
      root.render(<SessionTabList />);
    });
  }

  function groupLabels(): string[] {
    return Array.from(container.querySelectorAll('.kamux-tablist__group-label')).map(
      (el) => el.textContent ?? '',
    );
  }

  function sessionIdsInGroup(label: string): string[] {
    const labelEl = Array.from(container.querySelectorAll('.kamux-tablist__group-label')).find(
      (el) => el.textContent === label,
    );
    const group = labelEl?.closest('.kamux-tablist__group');
    return Array.from(group?.querySelectorAll('[data-session-id]') ?? []).map(
      (el) => el.getAttribute('data-session-id') ?? '',
    );
  }

  it('is_scratch で SESSIONS と SCRATCH に振り分ける（両方向）', () => {
    const s1 = session('s1');
    const s2 = { ...session('s2'), is_scratch: true };
    useAppStore.setState({
      sessions: { s1, s2 },
      sessionOrder: { backlog: [], in_progress: ['s1', 's2'], review: [], done: [] },
    });
    renderTabs();

    // SCRATCH には s2 が居る
    expect(sessionIdsInGroup('SCRATCH')).toEqual(['s2']);
    // SESSIONS には s2 が居ない（逆方向も確認しないと「常に両方描く」変異と潰れる）
    expect(sessionIdsInGroup('SESSIONS')).toEqual(['s1']);
  });

  it('SCRATCH が 0 件のときは見出しごと描かない', () => {
    useAppStore.setState({
      sessions: { s1: session('s1') },
      sessionOrder: { backlog: [], in_progress: ['s1'], review: [], done: [] },
    });
    renderTabs();

    expect(groupLabels()).toEqual(['SESSIONS']);
    expect(container.querySelectorAll('.kamux-tablist__group')).toHaveLength(1);
  });

  it('SESSIONS が 0 件のときは見出しごと描かない', () => {
    const s2 = { ...session('s2'), is_scratch: true };
    useAppStore.setState({
      sessions: { s2 },
      sessionOrder: { backlog: [], in_progress: ['s2'], review: [], done: [] },
    });
    renderTabs();

    expect(groupLabels()).toEqual(['SCRATCH']);
    expect(container.querySelectorAll('.kamux-tablist__group')).toHaveLength(1);
  });

  it('role="tablist" は 1 つのまま（グループがロールを割らない）', () => {
    const s1 = session('s1');
    const s2 = { ...session('s2'), is_scratch: true };
    useAppStore.setState({
      sessions: { s1, s2 },
      sessionOrder: { backlog: [], in_progress: ['s1', 's2'], review: [], done: [] },
    });
    renderTabs();

    expect(container.querySelectorAll('[role="tablist"]')).toHaveLength(1);
    expect(container.querySelectorAll('[role="tab"]')).toHaveLength(2);
    // グループの入れ物 div は a11y ツリーから透過させ、tab が tablist の
    // 直接の子として扱われる状態に戻す（role="presentation"）
    const groups = container.querySelectorAll('.kamux-tablist__group');
    expect(groups).toHaveLength(2);
    groups.forEach((group) => {
      expect(group.getAttribute('role')).toBe('presentation');
    });
  });

  it('グループの並びは SESSIONS が先、SCRATCH が後', () => {
    const s1 = session('s1');
    const s2 = { ...session('s2'), is_scratch: true };
    useAppStore.setState({
      sessions: { s1, s2 },
      sessionOrder: { backlog: [], in_progress: ['s1', 's2'], review: [], done: [] },
    });
    renderTabs();

    expect(groupLabels()).toEqual(['SESSIONS', 'SCRATCH']);
  });

  it('split2 でタブをクリックすると activePane 側のスロットへ割り当てる', () => {
    const s1 = session('s1');
    const s2 = session('s2');
    useAppStore.setState({
      sessions: { s1, s2 },
      sessionOrder: { backlog: [], in_progress: ['s1', 's2'], review: [], done: [] },
      layout: 'split2',
      activePane: 1,
      paneAssignment: [null, 's1'],
    });
    renderTabs();

    const tab = container.querySelector('[data-session-id="s2"]');
    act(() => {
      (tab as HTMLButtonElement).click();
    });

    // クリックは activePane（この場合 1）のスロットへ割り当てる。
    // pane 引数を取り違えて 0 固定にすると s1 が上書きされ、
    // 1 固定にすると常に正しく見えてしまうため、両スロットを見る。
    // 空いているペイン(0)へ入れる実装との区別のため、0 側は埋まっていない
    // （null のまま）ことも見る。
    expect(useAppStore.getState().paneAssignment[1]).toBe('s2');
    expect(useAppStore.getState().paneAssignment[0]).toBeNull();
  });

  it('split2 で activePane が 0 のときは 0 側のスロットへ割り当てる（1 固定の取り違えを検出する対称ケース）', () => {
    const s1 = session('s1');
    const s2 = session('s2');
    useAppStore.setState({
      sessions: { s1, s2 },
      sessionOrder: { backlog: [], in_progress: ['s1', 's2'], review: [], done: [] },
      layout: 'split2',
      activePane: 0,
      paneAssignment: [null, 's2'],
    });
    renderTabs();

    const tab = container.querySelector('[data-session-id="s1"]');
    act(() => {
      (tab as HTMLButtonElement).click();
    });

    expect(useAppStore.getState().paneAssignment[0]).toBe('s1');
    expect(useAppStore.getState().paneAssignment[1]).toBe('s2');
  });
});
