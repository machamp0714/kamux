import { useEffect, useState } from 'react';

import { bootstrap, useAppStore } from './store';
import { KANBAN_STATUSES, type AppError } from './types/model';

const errorMessage = (e: unknown): string => {
  const appError = e as Partial<AppError>;
  return appError?.code ? `[${appError.code}] ${appError.message}` : String(e);
};

export default function App() {
  const { projects, activeProjectId, sessions, sessionOrder } = useAppStore();
  const setActiveProject = useAppStore((s) => s.setActiveProject);
  const addProject = useAppStore((s) => s.addProject);
  const addSession = useAppStore((s) => s.addSession);
  const moveCard = useAppStore((s) => s.moveCard);

  const [error, setError] = useState<string | null>(null);
  const [projectName, setProjectName] = useState('');
  const [repoPath, setRepoPath] = useState('');
  const [sessionTitle, setSessionTitle] = useState('');

  useEffect(() => {
    bootstrap().catch((e) => setError(errorMessage(e)));
  }, []);

  const run = (task: () => Promise<unknown>) => {
    setError(null);
    task().catch((e) => setError(errorMessage(e)));
  };

  return (
    <main style={{ fontFamily: 'system-ui', padding: 16 }}>
      <h1>kamux</h1>
      {error && <p style={{ color: 'crimson' }}>{error}</p>}

      <section>
        <h2>Projects</h2>
        <ul>
          {projects.map((p) => (
            <li key={p.id}>
              <button
                onClick={() => run(() => setActiveProject(p.id))}
                style={{ fontWeight: p.id === activeProjectId ? 700 : 400 }}
              >
                {p.name}
              </button>
              <span> {p.repo_path}</span>
            </li>
          ))}
        </ul>
        <input
          placeholder="name"
          value={projectName}
          onChange={(e) => setProjectName(e.target.value)}
        />
        <input
          placeholder="/absolute/path/to/repo"
          value={repoPath}
          onChange={(e) => setRepoPath(e.target.value)}
        />
        <button
          onClick={() =>
            run(async () => {
              const created = await addProject(projectName, repoPath, 'claude');
              setProjectName('');
              setRepoPath('');
              await setActiveProject(created.id);
            })
          }
        >
          Add project
        </button>
      </section>

      <section>
        <h2>Sessions</h2>
        <input
          placeholder="session title"
          value={sessionTitle}
          onChange={(e) => setSessionTitle(e.target.value)}
        />
        <button
          disabled={!activeProjectId}
          onClick={() =>
            run(async () => {
              if (!activeProjectId) return;
              await addSession({
                projectId: activeProjectId,
                title: sessionTitle,
                description: '',
                mode: 'in_place',
                branch: null,
                cliKind: 'claude',
                cliCommand: null,
              });
              setSessionTitle('');
            })
          }
        >
          Add session
        </button>

        <div style={{ display: 'flex', gap: 16, marginTop: 16 }}>
          {KANBAN_STATUSES.map((status) => (
            <div key={status} style={{ flex: 1, border: '1px solid #ccc', padding: 8 }}>
              <h3>{status}</h3>
              <ul>
                {sessionOrder[status].map((id) => (
                  <li key={id}>
                    {sessions[id].title} ({sessions[id].sort_order})
                    <button
                      onClick={() =>
                        run(() =>
                          moveCard(
                            id,
                            KANBAN_STATUSES[
                              (KANBAN_STATUSES.indexOf(status) + 1) % KANBAN_STATUSES.length
                            ],
                            0,
                          ),
                        )
                      }
                    >
                      →
                    </button>
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>
      </section>
    </main>
  );
}
