import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';

/**
 * `ProjectSwitcher` は `App.tsx` で `SessionFormModal` と兄弟になる。どちらのスクリムも
 * 同じ `--z-scrim` を使う（`src/styles/tokens.css` が重なり順の正典。契約 §145.2）ので、
 * 前後関係は DOM の tree order だけで決まる —— 同一スタッキングレベルでは後に来た要素が前面。
 *
 * 12-C（コミット `698f9fb`）で `openModal` / `openCleanupDialog` / `setProjectSwitcherOpen` /
 * `openDeleteProjectDialog` の 4 つの opener が相互排他で全方向閉じた（`src/store/uiSlice.ts`
 * の各 `set({...})` を実際に読んで確認した —— `openModal` は `projectSwitcherOpen: false` を
 * 含む。`src/store/uiSlice.overlayExclusion.test.ts` の
 * `it('openModal は projectSwitcherOpen を落とす')` が実測している）。
 * 「スイッチャーを開いたまま Cmd+N」で 2 枚になる経路はもう無い。
 *
 * それでも `<SessionFormModal />` を `<ProjectSwitcherContainer />` より後にマウントする
 * 順序を固定するのは、契約 §145.4 が「順序が意味を持つ対を作ったら、その順序を固定する
 * テストを 1 本置くこと」を正典としているためである —— 新しいオーバーレイが足されたときに
 * 再びこの順序が意味を持ちうる。
 *
 * 固定できるのは `App.tsx` の JSX の並びだけである（両方 sentinel モックに差し替えるため、
 * 実 CSS も実描画も見ていない）。2 つを入れ替える変異をこのテストが赤にすることは
 * 変異検証で確認した —— 先例は `src/views/KanbanView/index.test.tsx` の兄弟順テスト。
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
