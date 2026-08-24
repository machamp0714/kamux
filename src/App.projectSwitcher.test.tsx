import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';

/**
 * `ProjectSwitcher` は `App.tsx` で `SessionFormModal` と兄弟になる。どちらのスクリムも
 * 同じ `--z-scrim` を使う（`src/styles/tokens.css` が重なり順の正典。契約 §145.2）ので、
 * 前後関係は DOM の tree order だけで決まる —— 同一スタッキングレベルでは後に来た要素が前面。
 *
 * 2 枚が同時に開く経路は残っている: `setProjectSwitcherOpen(true)` は `modal` を倒すが、
 * 逆向きの `openModal` は `projectSwitcherOpen` を倒さない（`openModal` は本タスクの射程外。
 * 契約 §145.5 が機構として記録している）。つまり「スイッチャーを開いたまま Cmd+N」で
 * 2 枚になる。そのとき後から開いた `SessionFormModal` が前面に来る必要があるため、
 * `<SessionFormModal />` を `<ProjectSwitcherContainer />` より後にマウントする。
 *
 * 固定できるのは `App.tsx` の JSX の並びだけである（両方 sentinel モックに差し替えるため、
 * 実 CSS も実描画も見ていない）。マウント順を入れ替える変異はこのテストが無いと全緑になる
 * —— 先例は `src/views/KanbanView/index.test.tsx` の兄弟順テスト（契約 §145.4）。
 */
vi.mock('./components/ErrorToast', () => ({ ErrorToast: () => null }));
vi.mock('./components/ProjectBar', () => ({ ProjectBar: () => null }));
vi.mock('./views/EditorView', () => ({ EditorView: () => null }));
vi.mock('./views/KanbanView', () => ({ KanbanView: () => null }));
vi.mock('./views/shared/NotificationPermissionBanner', () => ({
  NotificationPermissionBanner: () => null,
}));
vi.mock('./views/TerminalView', () => ({ TerminalView: () => null }));
vi.mock('./hooks/useKeymap', () => ({ useKeymap: vi.fn() }));
vi.mock('./hooks/useRuntimeStateEvents', () => ({ useRuntimeStateEvents: vi.fn() }));
vi.mock('./hooks/useFocusDeepLink', () => ({ useFocusDeepLink: vi.fn() }));
vi.mock('./hooks/useVisibilityContext', () => ({ useVisibilityContext: vi.fn() }));

// 順序を測る 2 つだけ sentinel を持たせる。
vi.mock('./views/KanbanView/SessionFormModal', () => ({
  SessionFormModal: () => <div data-testid="session-form-modal" />,
}));
vi.mock('./views/ProjectSwitcher/ProjectSwitcherContainer', () => ({
  ProjectSwitcherContainer: () => <div data-testid="project-switcher-container" />,
}));

vi.mock('./store', () => ({
  bootstrap: vi.fn().mockResolvedValue(undefined),
  useAppStore: (selector: (s: unknown) => unknown) =>
    selector({ view: 'kanban', setError: vi.fn(), sessions: {} }),
}));

import App from './App';

describe('App のオーバーレイのマウント順', () => {
  it('SessionFormModal は ProjectSwitcherContainer より後ろにマウントされる', () => {
    render(<App />);

    const switcher = screen.getByTestId('project-switcher-container');
    const modal = screen.getByTestId('session-form-modal');

    // DOCUMENT_POSITION_FOLLOWING は「modal が switcher より tree order で後続」を意味する。
    // 同一 z-index なので、後続であることが前面に来る条件そのもの。
    expect(switcher.compareDocumentPosition(modal) & Node.DOCUMENT_POSITION_FOLLOWING).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );
  });
});
