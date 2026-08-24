import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { render, screen } from '@testing-library/react';

// 子ビュー/コンポーネントは中身を持たないスタブへ差し替える。
// 本テストの関心は「ルートがどのフックを呼ぶか」であって描画内容ではない。
vi.mock('./components/ErrorToast', () => ({ ErrorToast: () => null }));
vi.mock('./components/ProjectBar', () => ({ ProjectBar: () => null }));
vi.mock('./views/EditorView', () => ({ EditorView: () => null }));
vi.mock('./views/KanbanView', () => ({ KanbanView: () => null }));
vi.mock('./views/KanbanView/SessionFormModal', () => ({ SessionFormModal: () => null }));
// バナーだけ sentinel を持たせる。マウント配線（App.tsx の JSX）を観測するため。
vi.mock('./views/shared/NotificationPermissionBanner', () => ({
  NotificationPermissionBanner: () => <div data-testid="notification-permission-banner" />,
}));
vi.mock('./views/TerminalView', () => ({ TerminalView: () => null }));
vi.mock('./hooks/useKeymap', () => ({ useKeymap: vi.fn() }));
vi.mock('./hooks/useRuntimeStateEvents', () => ({ useRuntimeStateEvents: vi.fn() }));

const reportFrontendReadySpy = vi.fn().mockResolvedValue(undefined);
vi.mock('./ipc/commands', () => ({
  reportFrontendReady: () => reportFrontendReadySpy(),
}));

/**
 * `requestAnimationFrame` / `cancelAnimationFrame` のフェイク。
 * `src/views/TerminalView/TerminalGrid.test.tsx` の `fakeRaf`（同ファイル 109 行目、
 * コメントは 100-108 行目）と同じ形。壁時計に依存せず手動でキューを進める。
 */
function fakeRaf() {
  const queue = new Map<number, () => void>();
  let id = 0;
  return {
    raf: (cb: () => void): number => {
      id += 1;
      queue.set(id, cb);
      return id;
    },
    caf: (h: number): void => {
      queue.delete(h);
    },
    tick: (): void => {
      const cbs = [...queue.values()];
      queue.clear();
      cbs.forEach((cb) => cb());
    },
  };
}

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
    expect(screen.getByTestId('notification-permission-banner')).toBeInTheDocument();
  });

  describe('起動時間計装（Task 13(ii)、契約 §0 の起動時間測定点）', () => {
    let raf: ReturnType<typeof fakeRaf>;

    beforeEach(() => {
      reportFrontendReadySpy.mockClear();
      raf = fakeRaf();
      vi.stubGlobal('requestAnimationFrame', raf.raf);
      vi.stubGlobal('cancelAnimationFrame', raf.caf);
    });

    afterEach(() => {
      vi.unstubAllGlobals();
    });

    it('初回ペイント後（rAF コールバック）に reportFrontendReady をちょうど1回呼ぶ', () => {
      const { rerender } = render(<App />);
      // マウント直後はまだ rAF のコールバックが走っていないので呼ばれていないはず。
      expect(reportFrontendReadySpy).not.toHaveBeenCalled();

      raf.tick();
      expect(reportFrontendReadySpy).toHaveBeenCalledTimes(1);

      // 依存配列が [] であれば rerender では再実行されない。tick まで進めて
      // キューに積まれていないことを確認する（tick しないと、rerender で新しい
      // rAF が積まれていても未実行のまま「1回」に見えてしまう）。
      rerender(<App />);
      raf.tick();
      expect(reportFrontendReadySpy).toHaveBeenCalledTimes(1);
    });

    it('アンマウント時に cancelAnimationFrame でペンディングの通知を止める', () => {
      const { unmount } = render(<App />);
      unmount();
      raf.tick();
      expect(reportFrontendReadySpy).not.toHaveBeenCalled();
    });
  });
});
