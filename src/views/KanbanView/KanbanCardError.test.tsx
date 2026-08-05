import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { KanbanCardError } from './KanbanCardError';
import { useAppStore } from '../../store';

// autoCleanup は vitest の globals が無効なので登録されない。明示的に片付ける
afterEach(cleanup);

beforeEach(() => {
  useAppStore.setState({ runtimeStates: {}, runtimeReasons: {}, runtimeErrors: {} });
});

describe('KanbanCardError（契約 §42.4）', () => {
  it('error のときは生 stderr を原文のまま出す', () => {
    const raw = 'command not found: claude\n  at spawn (2 行目)';
    useAppStore.setState({ runtimeStates: { s1: 'error' }, runtimeErrors: { s1: raw } });

    const { container } = render(<KanbanCardError sessionId="s1" />);

    // 既定の normalizer は改行と連続空白を潰す。原文のままであることを見たいので無効化する
    const el = screen.getByText(raw, { normalizer: (s) => s });
    expect(el).toHaveClass('kanban-card__error');
    // 加工しない（前置きも省略記号も足さない）
    expect(el.textContent).toBe(raw);
    expect(container.childElementCount).toBe(1);
  });

  it('error 以外ならメッセージが残っていても描かない', () => {
    useAppStore.setState({ runtimeStates: { s1: 'running' }, runtimeErrors: { s1: 'boom' } });

    const { container } = render(<KanbanCardError sessionId="s1" />);

    expect(container).toBeEmptyDOMElement();
  });

  // ❌ だがメッセージがまだ無い一瞬（イベントが catch より先に着いた場合）に空枠を描かない
  it('error でもメッセージが無ければ空枠を描かない', () => {
    useAppStore.setState({ runtimeStates: { s1: 'error' }, runtimeErrors: {} });

    const { container } = render(<KanbanCardError sessionId="s1" />);

    expect(container).toBeEmptyDOMElement();
  });

  it('他セッションのエラーは描かない', () => {
    useAppStore.setState({ runtimeStates: { other: 'error' }, runtimeErrors: { other: 'boom' } });

    const { container } = render(<KanbanCardError sessionId="s1" />);

    expect(container).toBeEmptyDOMElement();
  });
});
