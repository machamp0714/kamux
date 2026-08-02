import { useState } from 'react';

import { useAppStore } from '../store';
import { toAppError } from '../store/uiSlice';
import type { CliKind } from '../types/model';
import './ProjectBar.css';

const CLI_KINDS: CliKind[] = ['claude', 'codex', 'shell', 'custom'];

/**
 * 契約 §49.7: 責務は `setActiveProject` による選択と `addProject` による作成の
 * 2 つだけ。M1-1 Task 18 が `App.tsx` にインラインで置いた暫定 UI を移したもので、
 * 新機能ではない。削除の導線はここに足さない（M3-4 Task 12 の担当）。
 */
export function canSubmitProjectForm(values: { name: string; repoPath: string }): boolean {
  return values.name.trim() !== '' && values.repoPath.trim() !== '';
}

export function ProjectBar() {
  const projects = useAppStore((s) => s.projects);
  const activeProjectId = useAppStore((s) => s.activeProjectId);
  const setActiveProject = useAppStore((s) => s.setActiveProject);
  const addProject = useAppStore((s) => s.addProject);
  const setError = useAppStore((s) => s.setError);

  const [name, setName] = useState('');
  const [repoPath, setRepoPath] = useState('');
  const [defaultCli, setDefaultCli] = useState<CliKind>('claude');
  const [busy, setBusy] = useState(false);

  const canSubmit = canSubmitProjectForm({ name, repoPath }) && !busy;

  const onSubmit = async () => {
    if (!canSubmit) return;
    setBusy(true);
    try {
      const created = await addProject(name.trim(), repoPath.trim(), defaultCli);
      setName('');
      setRepoPath('');
      setDefaultCli('claude');
      await setActiveProject(created.id);
    } catch (e: unknown) {
      setError(toAppError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="project-bar">
      <ul className="project-bar__list">
        {projects.map((p) => (
          <li key={p.id}>
            <button
              type="button"
              className={
                p.id === activeProjectId
                  ? 'project-bar__item project-bar__item--active'
                  : 'project-bar__item'
              }
              onClick={() => {
                setActiveProject(p.id).catch((e: unknown) => setError(toAppError(e)));
              }}
            >
              {p.name}
            </button>
          </li>
        ))}
      </ul>

      <form
        className="project-bar__form"
        onSubmit={(e) => {
          e.preventDefault();
          void onSubmit();
        }}
      >
        <input
          className="project-bar__input"
          placeholder="name"
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <input
          className="project-bar__input project-bar__input--mono"
          placeholder="/absolute/path/to/repo"
          value={repoPath}
          onChange={(e) => setRepoPath(e.target.value)}
        />
        <select
          className="project-bar__select"
          value={defaultCli}
          onChange={(e) => setDefaultCli(e.target.value as CliKind)}
        >
          {CLI_KINDS.map((kind) => (
            <option key={kind} value={kind}>
              {kind}
            </option>
          ))}
        </select>
        <button type="submit" className="project-bar__submit" disabled={!canSubmit}>
          Add project
        </button>
      </form>
    </div>
  );
}
