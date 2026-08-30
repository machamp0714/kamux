import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { DndContext, KeyboardSensor, PointerSensor, useSensor, useSensors } from '@dnd-kit/core';
import { SortableContext, sortableKeyboardCoordinates } from '@dnd-kit/sortable';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// React 18 の act() は既定でこのフラグを見て、テスト環境かどうかを判定する
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

import { useAppStore } from '../../store';
import type { Session } from '../../types/model';
import { KanbanCard } from './KanbanCard';
import { KANBAN_KEYBOARD_CODES, KANBAN_POINTER_ACTIVATION_DISTANCE } from './sensors';
import { SortableCard } from './SortableCard';

function session(id: string): Session {
  return {
    id,
    project_id: 'p1',
    title: id,
    description: '',
    kanban_status: 'backlog',
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

class FakeResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', FakeResizeObserver);
  useAppStore.setState({
    view: 'kanban',
    focusedSessionId: null,
    activePane: 0,
    paneAssignment: [null, null],
  });
  container = document.createElement('div');
  document.body.appendChild(container);
});

afterEach(() => {
  act(() => {
    root.unmount();
  });
  container.remove();
  vi.unstubAllGlobals();
});

function card(): HTMLElement {
  const el = container.querySelector<HTMLElement>('.kanban-card');
  if (el === null) throw new Error('.kanban-card not rendered');
  return el;
}

function pressEnter(el: HTMLElement): void {
  act(() => {
    el.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', code: 'Enter', bubbles: true }));
  });
}

describe('KanbanCard（要件5: クリック / Enter で開く）', () => {
  it('カードをクリックすると onOpen がセッション ID で呼ばれる', () => {
    const onOpen = vi.fn();
    act(() => {
      root = createRoot(container);
      root.render(<KanbanCard session={session('s1')} onOpen={onOpen} />);
    });

    act(() => {
      card().click();
    });

    expect(onOpen).toHaveBeenCalledWith('s1');
  });

  it('カード上の Enter で onOpen が呼ばれる', () => {
    const onOpen = vi.fn();
    act(() => {
      root = createRoot(container);
      root.render(<KanbanCard session={session('s1')} onOpen={onOpen} />);
    });

    pressEnter(card());

    expect(onOpen).toHaveBeenCalledWith('s1');
  });

  it('編集 / アーカイブのクリックはカードを開かない', () => {
    const onOpen = vi.fn();
    const onEdit = vi.fn();
    act(() => {
      root = createRoot(container);
      root.render(
        <KanbanCard
          session={session('s1')}

          onOpen={onOpen}
          onEdit={onEdit}
          onArchive={vi.fn()}
        />,
      );
    });

    const edit = container.querySelector<HTMLButtonElement>('.kanban-card__actions button');
    act(() => {
      edit?.click();
    });

    expect(onEdit).toHaveBeenCalledWith('s1');
    expect(onOpen).not.toHaveBeenCalled();
  });

  it('onOpen が無いクローン（DragOverlay）はタブストップにならない', () => {
    act(() => {
      root = createRoot(container);
      root.render(<KanbanCard session={session('s1')} />);
    });

    expect(card().hasAttribute('tabindex')).toBe(false);
  });
});

// KanbanCardResume は葉として自分で購読するので（契約 §25.5 / §38.3）、KanbanCard の
// 実物を render しないと「差し込みの 1 行」の欠落を検出できない（契約 §81 群 S）。
describe('KanbanCard と KanbanCardResume の配線（第1部 §4.4）', () => {
  beforeEach(() => {
    useAppStore.setState({ sessions: { s1: session('s1') }, resumeFailedSessionIds: [] });
  });

  it('runtimeStates が interrupted のとき再開ボタンが actions 内に描かれる', () => {
    useAppStore.setState({ runtimeStates: { s1: 'interrupted' } });
    act(() => {
      root = createRoot(container);
      root.render(<KanbanCard session={session('s1')} onOpen={vi.fn()} />);
    });

    // session('s1') は cli_kind: 'shell' なので resumeAffordance のラベルは
    // 「プロセスを再起動」（claude 以外の分岐。src/session/resumeAffordance.ts）
    const actions = card().querySelector('.kanban-card__actions');
    const buttons = [...(actions?.querySelectorAll('button') ?? [])].map((b) => b.textContent);
    expect(buttons).toContain('プロセスを再起動');
  });

  it('runtimeStates が running のときは再開ボタンを描かない', () => {
    useAppStore.setState({ runtimeStates: { s1: 'running' } });
    act(() => {
      root = createRoot(container);
      root.render(<KanbanCard session={session('s1')} onOpen={vi.fn()} />);
    });

    const actions = card().querySelector('.kanban-card__actions');
    const buttons = [...(actions?.querySelectorAll('button') ?? [])].map((b) => b.textContent);
    expect(buttons).not.toContain('プロセスを再起動');
  });

  // Important 2: DragOverlay のクローン（onOpen も dragActivator も渡さない。
  // KanbanView/index.tsx:139 の形）に、破壊的操作を起こせる生きたボタンが
  // 入ってはならない（KanbanCard.tsx:45-47 の doc の不変条件）。
  it('クローン形状（onOpen も dragActivator も無い）では再開ボタンが存在しない', () => {
    useAppStore.setState({ runtimeStates: { s1: 'interrupted' } });
    act(() => {
      root = createRoot(container);
      root.render(<KanbanCard session={session('s1')} />);
    });

    const actions = card().querySelector('.kanban-card__actions');
    const buttons = [...(actions?.querySelectorAll('button') ?? [])].map((b) => b.textContent);
    expect(buttons).not.toContain('プロセスを再起動');
  });

  it('実カード形状（onOpen を渡す）では再開ボタンが存在する', () => {
    useAppStore.setState({ runtimeStates: { s1: 'interrupted' } });
    act(() => {
      root = createRoot(container);
      root.render(<KanbanCard session={session('s1')} onOpen={vi.fn()} />);
    });

    const actions = card().querySelector('.kanban-card__actions');
    const buttons = [...(actions?.querySelectorAll('button') ?? [])].map((b) => b.textContent);
    expect(buttons).toContain('プロセスを再起動');
  });
});

// KanbanCardCleanup も葉として自分で購読する（契約 §25.5 / §38.3）。差し込みの 1 行が
// 落ちても葉のテストは緑のままなので、KanbanCard の実物を render して呼び出し側を見る。
describe('KanbanCard と KanbanCardCleanup の配線（M3-4 Task 9）', () => {
  // KanbanCardResume の describe と同じ流儀で、この describe 用のストアを毎回組み直す。
  // zustand の setState はファイル内で永続するため、置きっぱなしにすると後続の
  // describe が掃除ボタンのモックを持ったまま走る。
  beforeEach(() => {
    useAppStore.setState({ sessions: {}, openCleanupDialog: vi.fn(async () => undefined) });
  });

  function cleanupTarget(id: string): Session {
    return {
      ...session(id),
      mode: 'worktree',
      branch: 'session/fix-login',
      worktree_path: '/repo/a/.worktrees/session-fix-login',
      kanban_status: 'done',
    };
  }

  function actionLabels(): (string | null)[] {
    const actions = card().querySelector('.kanban-card__actions');
    return [...(actions?.querySelectorAll('button') ?? [])].map((b) =>
      b.getAttribute('aria-label'),
    );
  }

  it('掃除を提案すべきセッションでは掃除ボタンが actions 内に描かれる', () => {
    const s = cleanupTarget('s1');
    useAppStore.setState({ sessions: { s1: s } });
    act(() => {
      root = createRoot(container);
      root.render(<KanbanCard session={s} onOpen={vi.fn()} />);
    });

    expect(actionLabels()).toContain('worktree を掃除');
  });

  it('掃除を提案しないセッション（in_place）では掃除ボタンを描かない', () => {
    const s = session('s1');
    useAppStore.setState({ sessions: { s1: s } });
    act(() => {
      root = createRoot(container);
      root.render(<KanbanCard session={s} onOpen={vi.fn()} />);
    });

    expect(actionLabels()).not.toContain('worktree を掃除');
  });

  // KanbanCardResume と同じ理由: DragOverlay のクローンに破壊的操作を起こせる
  // 生きたボタンが入ってはならない（KanbanCard.tsx の doc の不変条件）。
  it('クローン形状（onOpen も dragActivator も無い）では掃除ボタンが存在しない', () => {
    const s = cleanupTarget('s1');
    useAppStore.setState({ sessions: { s1: s } });
    act(() => {
      root = createRoot(container);
      root.render(<KanbanCard session={s} />);
    });

    expect(actionLabels()).not.toContain('worktree を掃除');
  });

  it('掃除ボタンを押すとそのカードのセッション ID で openCleanupDialog が呼ばれる', () => {
    const openCleanupDialog = vi.fn(async () => undefined);
    const s = cleanupTarget('s2');
    useAppStore.setState({ sessions: { s2: s }, openCleanupDialog });
    act(() => {
      root = createRoot(container);
      root.render(<KanbanCard session={s} onOpen={vi.fn()} />);
    });

    const button = card().querySelector<HTMLButtonElement>('[aria-label="worktree を掃除"]');
    if (button === null) throw new Error('掃除ボタンが描かれていない');
    act(() => {
      button.click();
    });

    expect(openCleanupDialog).toHaveBeenCalledWith('s2');
  });
});

// components.md「カード」節: kanban-card__head は「横並び・両端寄せ。左に CLI チップ、
// 右にバッジ」= 2 グループである。ここに 3 要素目（title）が混ざると
// justify-content: space-between の意味が変わり、バッジが中央に落ちる。
// CSS では検出できない構造なのでマークアップ側で固定する。
describe('KanbanCard の head（components.md「カード」節）', () => {
  beforeEach(() => {
    act(() => {
      root = createRoot(container);
      root.render(<KanbanCard session={session('s1')} />);
    });
  });

  it('head は左に CLI チップ・右にバッジの 2 要素だけを持つ', () => {
    const head = card().querySelector('.kanban-card__head');
    const children = [...(head?.children ?? [])].map((el) => el.className);
    expect(children).toEqual(['kanban-card__cli', 'kanban-card__badge']);
  });

  it('title は head の中ではなくカード直下の子要素である', () => {
    const title = card().querySelector('.kanban-card__title');
    expect(title?.parentElement).toBe(card());
  });
});

// タブストップの不変条件は「dnd-kit の attributes を載せた後の木」にしか存在しないため、
// KanbanCard 単体ではなく SortableCard で合成した状態を見る（Important 2 のリグレッション）。
describe('SortableCard 合成時（カード 1 枚あたりのタブストップ）', () => {
  /**
   * センサ設定は KanbanView と同じ定数（sensors.ts）から組む。dnd-kit の既定の
   * KeyboardCodes は start に Enter を含むため、既定のまま貼ると Enter が
   * ドラッグ開始に食われて「開く」の検証にならない。
   */
  function Board({ onDragStart }: { onDragStart?: () => void }): JSX.Element {
    const sensors = useSensors(
      useSensor(PointerSensor, {
        activationConstraint: { distance: KANBAN_POINTER_ACTIVATION_DISTANCE },
      }),
      useSensor(KeyboardSensor, {
        coordinateGetter: sortableKeyboardCoordinates,
        keyboardCodes: KANBAN_KEYBOARD_CODES,
      }),
    );
    return (
      <DndContext sensors={sensors} onDragStart={onDragStart}>
        <SortableContext items={['s1']}>
          <SortableCard session={session('s1')} />
        </SortableContext>
      </DndContext>
    );
  }

  function renderSortable(onDragStart?: () => void): void {
    act(() => {
      root = createRoot(container);
      root.render(<Board onDragStart={onDragStart} />);
    });
  }

  it('タブストップはカード自身の 1 つだけで、dnd-kit のラッパには tabindex が付かない', () => {
    renderSortable();

    const stops = container.querySelectorAll('[tabindex]');
    expect(stops).toHaveLength(1);
    expect(stops[0]).toBe(card());
    expect(container.querySelector('.kanban-sortable')?.hasAttribute('tabindex')).toBe(false);
  });

  it('そのタブストップにフォーカスするとカードが :focus-within の対象になる', () => {
    renderSortable();

    act(() => {
      card().focus();
    });

    // アクションを出す CSS は .kanban-card:focus-within。フォーカスがカード自身
    // （またはその内側）にあることが条件で、ラッパにあると一致しない。
    expect(document.activeElement).toBe(card());
  });

  it('カードをクリックするとターミナル画面へ切り替わり、アクティブペインに載る', () => {
    renderSortable();

    act(() => {
      card().click();
    });

    const s = useAppStore.getState();
    expect(s.view).toBe('terminal');
    expect(s.focusedSessionId).toBe('s1');
    expect(s.paneAssignment[0]).toBe('s1');
  });

  it('同じタブストップ上の Space はドラッグ開始のままで、カードを開かない', () => {
    const onDragStart = vi.fn();
    renderSortable(onDragStart);

    act(() => {
      card().focus();
      card().dispatchEvent(
        new KeyboardEvent('keydown', { key: ' ', code: 'Space', bubbles: true }),
      );
    });

    expect(onDragStart).toHaveBeenCalled();
    expect(useAppStore.getState().view).toBe('kanban');
  });

  it('タブストップ上の Enter でもターミナル画面へ切り替わる', () => {
    renderSortable();

    act(() => {
      card().focus();
    });
    pressEnter(card());

    const s = useAppStore.getState();
    expect(s.view).toBe('terminal');
    expect(s.focusedSessionId).toBe('s1');
  });

  it('カードのルート要素は role="article" のままで、dnd-kit 既定の role="button" には戻らない', () => {
    // role='button' に戻ると「編集」「アーカイブ」の <button> が
    // children-presentational になり支援技術から消える。ここが崩れると
    // 症状が出ないまま dnd-kit の既定挙動（aria-pressed の付与含む）に戻る。
    renderSortable();

    expect(card().getAttribute('role')).toBe('article');
    expect(card().hasAttribute('aria-pressed')).toBe(false);
  });
});
