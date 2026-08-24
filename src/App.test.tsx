import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';

// このプロジェクトに @types/node は入っていない（フロントはブラウザ向け、tsconfig の
// lib も DOM 系のみ）。vitest 自体は Node 上で動くので `process` は実行時には存在する
// —— unhandled rejection を検出するテストのためだけに、使う分だけ最小限の型を宣言する。
declare const process: {
  on(
    event: 'unhandledRejection',
    listener: (reason: unknown, promise: Promise<unknown>) => void,
  ): void;
  off(
    event: 'unhandledRejection',
    listener: (reason: unknown, promise: Promise<unknown>) => void,
  ): void;
};

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
// tinyspy（vi.fn の内部実装、node_modules/tinyspy/dist/index.js）は
// スパイが返す Promise に呼び出し直後 `p.then(...)` を無条件で付けて resolves を記録する。
// そのため reportFrontendReadySpy() の返り値をそのまま App.tsx へ渡すと、reject させても
// tinyspy 側の内部ハンドラが既に付いており、Node の unhandled rejection 検出が発火しない
// （「モックが実際に reject していること」を測れなくなる）。呼び出し回数の記録はスパイに
// 任せつつ、App.tsx へ渡す Promise はスパイの外側で作った素の Promise にする。
let reportFrontendReadyRejects = false;
vi.mock('./ipc/commands', () => ({
  reportFrontendReady: () => {
    reportFrontendReadySpy();
    return reportFrontendReadyRejects
      ? Promise.reject(new Error('report_frontend_ready failed'))
      : Promise.resolve(undefined);
  },
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

  // このファイルは `vite.config.ts` の `test.globals`（未設定 = false）を前提に
  // `describe`/`it` などを明示 import している。そのため
  // `@testing-library/react` の自動 cleanup（globalThis.afterEach の検出に依存する）
  // は効かない —— 他ファイル（例: `src/components/ErrorToast.test.tsx`）と同じく
  // 明示的に `afterEach(cleanup)` を置く必要がある。
  //
  // これを怠ると、先行する `it('ルートで...')` がマウントした App のうち
  // rAF をスタブしていないもの（実 `requestAnimationFrame`、jsdom 内部実装は
  // `setTimeout` ベース）が、後続の非同期テスト（下の reject テストの
  // `await new Promise((resolve) => setTimeout(resolve, 0))`）の間に発火し、
  // `reportFrontendReadySpy` の呼び出し回数を予期せず増やしてフレークする
  // （修正ラウンド1で実測）。unmount すれば effect の cleanup（`cancelAnimationFrame`）
  // が効き、この経路を断てる。
  afterEach(cleanup);

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
      reportFrontendReadyRejects = false;
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

    it('reportFrontendReady が reject しても呼び出し側から拒否が漏れない（修正ラウンド1、CI フレークの再現）', async () => {
      // 実アプリでは Tauri IPC 未初期化時などに reject しうる（App.projectSwitcher.test.tsx
      // が `./ipc/commands` をモックしていないために起きた unhandled rejection のフレークを
      // ここで直接再現する）。
      reportFrontendReadyRejects = true;
      const onUnhandledRejection = vi.fn();
      process.on('unhandledRejection', onUnhandledRejection);
      try {
        render(<App />);
        raf.tick();
        // reject の伝播はマイクロタスクを跨ぐため、マクロタスク境界まで進めてから検査する。
        await new Promise((resolve) => setTimeout(resolve, 0));
        expect(reportFrontendReadySpy).toHaveBeenCalledTimes(1);
        expect(onUnhandledRejection).not.toHaveBeenCalled();
      } finally {
        process.off('unhandledRejection', onUnhandledRejection);
      }
    });
  });
});
