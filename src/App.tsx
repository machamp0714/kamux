import { useEffect, useMemo } from 'react';
import type { UnlistenFn } from '@tauri-apps/api/event';

import { ErrorToast } from './components/ErrorToast';
import { ProjectBar } from './components/ProjectBar';
import { useKeymap } from './hooks/useKeymap';
import { useRuntimeStateEvents } from './hooks/useRuntimeStateEvents';
import { listenFocus } from './ipc/events';
import { bootstrap, useAppStore } from './store';
import { selectSessionIdsKey } from './store/sessionSlice';
import { toAppError } from './store/uiSlice';
import { KanbanView } from './views/KanbanView';
import { SessionFormModal } from './views/KanbanView/SessionFormModal';
import { TerminalView } from './views/TerminalView';
import './App.css';

export default function App() {
  const view = useAppStore((s) => s.view);
  const setError = useAppStore((s) => s.setError);
  const focusSession = useAppStore((s) => s.focusSession);
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

  useEffect(() => {
    bootstrap().catch((e: unknown) => setError(toAppError(e)));
  }, [setError]);

  // focus://session/{session_id}（契約 §8）の受信。emit するのは M2-3（通知クリック）。
  // カードクリックと同じ focusSession に収束させる（要件5）。
  useEffect(() => {
    const unlistens: Promise<UnlistenFn>[] = sessionIds.map((id) =>
      listenFocus(id, (p) =>
        focusSession(p.session_id, p.surface_kind === 'editor' ? 'editor' : 'terminal'),
      ),
    );
    return () => {
      unlistens.forEach((u) => void u.then((fn) => fn()));
    };
  }, [sessionIds, focusSession]);

  return (
    <div className="app">
      <ProjectBar />
      {view === 'kanban' ? <KanbanView /> : null}
      {view === 'terminal' && <TerminalView />}
      {view === 'editor' ? (
        <div className="app__placeholder">エディタ画面は M3-1 で実装します</div>
      ) : null}
      <SessionFormModal />
      <ErrorToast />
    </div>
  );
}
