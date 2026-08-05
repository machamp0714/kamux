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
      root.render(<KanbanCard session={session('s1')} runtimeStates={{}} onOpen={onOpen} />);
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
      root.render(<KanbanCard session={session('s1')} runtimeStates={{}} onOpen={onOpen} />);
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
          runtimeStates={{}}
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
      root.render(<KanbanCard session={session('s1')} runtimeStates={{}} />);
    });

    expect(card().hasAttribute('tabindex')).toBe(false);
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
          <SortableCard session={session('s1')} runtimeStates={{}} />
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
