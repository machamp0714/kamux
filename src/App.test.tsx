import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render } from '@testing-library/react';

// 子ビュー/コンポーネントは中身を持たないスタブへ差し替える。
// 本テストの関心は「ルートがどのフックを呼ぶか」であって描画内容ではない。
vi.mock('./components/ErrorToast', () => ({ ErrorToast: () => null }));
vi.mock('./components/ProjectBar', () => ({ ProjectBar: () => null }));
vi.mock('./views/EditorView', () => ({ EditorView: () => null }));
vi.mock('./views/KanbanView', () => ({ KanbanView: () => null }));
vi.mock('./views/KanbanView/SessionFormModal', () => ({ SessionFormModal: () => null }));
vi.mock('./views/TerminalView', () => ({ TerminalView: () => null }));
vi.mock('./hooks/useKeymap', () => ({ useKeymap: vi.fn() }));
vi.mock('./hooks/useRuntimeStateEvents', () => ({ useRuntimeStateEvents: vi.fn() }));

const useFocusDeepLinkSpy = vi.fn();
vi.mock('./hooks/useFocusDeepLink', () => ({
  useFocusDeepLink: () => useFocusDeepLinkSpy(),
}));

const useVisibilityContextSpy = vi.fn();
vi.mock('./hooks/useVisibilityContext', () => ({
  useVisibilityContext: () => useVisibilityContextSpy(),
}));

const setError = vi.fn();
vi.mock('./store', () => ({
  bootstrap: vi.fn().mockResolvedValue(undefined),
  useAppStore: (selector: (s: unknown) => unknown) =>
    selector({ view: 'kanban', setError, sessions: {} }),
}));

import App from './App';

describe('App', () => {
  beforeEach(() => {
    useFocusDeepLinkSpy.mockClear();
    useVisibilityContextSpy.mockClear();
  });

  it('ルートで useFocusDeepLink と useVisibilityContext を呼ぶ（Ruling AE/AG の呼び出し側配線を守る）', () => {
    render(<App />);
    expect(useFocusDeepLinkSpy).toHaveBeenCalledTimes(1);
    expect(useVisibilityContextSpy).toHaveBeenCalledTimes(1);
  });
});
