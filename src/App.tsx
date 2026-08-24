import { useEffect, useMemo } from 'react';

import { ErrorToast } from './components/ErrorToast';
import { ProjectBar } from './components/ProjectBar';
import { useFocusDeepLink } from './hooks/useFocusDeepLink';
import { useKeymap } from './hooks/useKeymap';
import { useRuntimeStateEvents } from './hooks/useRuntimeStateEvents';
import { useVisibilityContext } from './hooks/useVisibilityContext';
import { reportFrontendReady } from './ipc/commands';
import { bootstrap, useAppStore } from './store';
import { selectSessionIdsKey } from './store/sessionSlice';
import { toAppError } from './store/uiSlice';
import { EditorView } from './views/EditorView';
import { KanbanView } from './views/KanbanView';
import { SessionFormModal } from './views/KanbanView/SessionFormModal';
import { ProjectSwitcherContainer } from './views/ProjectSwitcher/ProjectSwitcherContainer';
import { NotificationPermissionBanner } from './views/shared/NotificationPermissionBanner';
import { TerminalView } from './views/TerminalView';
import './App.css';

export default function App() {
  const view = useAppStore((s) => s.view);
  const setError = useAppStore((s) => s.setError);
  // セレクタはプリミティブを返す（sessions オブジェクト全体は select しない）。
  // ID の集合が変わったときだけ effect を再実行すればよいので、ソート済みの
  // カンマ区切り文字列にして依存値にする。
  const sessionIdsKey = useAppStore(selectSessionIdsKey);
  const sessionIds = useMemo(
    () => (sessionIdsKey === '' ? [] : sessionIdsKey.split(',')),
    [sessionIdsKey],
  );

  useKeymap();
  // session://state/{session_id}（契約 §8）の差分購読。ルートで 1 回だけ呼ぶ。
  useRuntimeStateEvents(sessionIds);
  // focus://session/{session_id}（契約 §8）の受信。emit するのは M2-3（通知クリック）。
  // カードクリックと同じ focusSession に収束させる（要件5）。
  useFocusDeepLink();
  // 表示中のビューとセッションを Rust に push する（M2-3、通知の前面抑制に使う）。
  useVisibilityContext();

  useEffect(() => {
    bootstrap().catch((e: unknown) => setError(toAppError(e)));
  }, [setError]);

  useEffect(() => {
    // requestAnimationFrame で最初のペイントが済んでから記録する（契約 §0 の起動時間
    // 測定点、M3-4 Task 13）。依存配列は [] —— 再レンダリングのたびに呼ばないこと。
    // `src-tauri/src/perf.rs` の「失敗しても計測のためだけの機能なのでアプリは止めない」
    // という設計思想をフロント側にも適用する（修正ラウンド1）。`.catch()` を外すと、
    // 拒否が unhandled rejection として漏れる（IPC 未初期化時などに実機でも起こりうる）。
    const id = requestAnimationFrame(() => {
      reportFrontendReady().catch(() => {});
    });
    return () => cancelAnimationFrame(id);
  }, []);

  return (
    <div className="app">
      <ProjectBar />
      <NotificationPermissionBanner />
      {view === 'kanban' ? <KanbanView /> : null}
      {view === 'terminal' && <TerminalView />}
      {view === 'editor' && <EditorView />}
      {/* 同じ --z-scrim を使う 2 枚。tree order が前後を決めるので順序を動かさない
          （src/App.projectSwitcher.test.tsx が固定している）。 */}
      <ProjectSwitcherContainer />
      <SessionFormModal />
      <ErrorToast />
    </div>
  );
}
