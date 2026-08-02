import { useEffect } from 'react';

import { ErrorToast } from './components/ErrorToast';
import { ProjectBar } from './components/ProjectBar';
import { useKeymap } from './hooks/useKeymap';
import { bootstrap, useAppStore } from './store';
import { KanbanView } from './views/KanbanView';
import { SessionFormModal } from './views/KanbanView/SessionFormModal';

export default function App() {
  const view = useAppStore((s) => s.view);

  useKeymap();

  useEffect(() => {
    void bootstrap();
  }, []);

  return (
    <div className="app">
      <ProjectBar />
      {view === 'kanban' ? <KanbanView /> : null}
      {view === 'terminal' ? (
        <div className="app__placeholder">ターミナル画面は M1-3 で実装します</div>
      ) : null}
      {view === 'editor' ? (
        <div className="app__placeholder">エディタ画面は M3-1 で実装します</div>
      ) : null}
      <SessionFormModal />
      <ErrorToast />
    </div>
  );
}
