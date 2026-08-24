import { useState } from 'react';

import { useAppStore } from '../store';
import { toAppError } from '../store/uiSlice';
import type { CliKind } from '../types/model';
import { DeleteProjectDialogContainer } from './DeleteProjectDialogContainer';
import './ProjectBar.css';

const CLI_KINDS: CliKind[] = ['claude', 'codex', 'shell', 'custom'];

/**
 * 契約 §49.7 / §130.3: 責務は `setActiveProject` による選択・`addProject` による作成・
 * `removeProject` による削除の 3 つ。選択と作成は M1-1 Task 18 が `App.tsx` にインラインで
 * 置いた暫定 UI を移したもので、新機能ではない。削除の導線は M3-4 Task 12 でここに着地した
 * —— 契約 §130.3 が「速度のための面（`ProjectSwitcher`）に破壊操作を混ぜない。既に作成
 * フォームと選択を持つ管理面である `ProjectBar` の隣に並べる」と定めたためである。
 * 押した瞬間には消さず、確認ダイアログ（`DeleteProjectDialogContainer`）を挟む（契約 §7.1）。
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
  // runtimeStates はここでは購読しない（購読者は DeleteProjectDialogContainer だけ。
  // 契約 §38.3 の許可リスト / §147.4）。
  const openDeleteProjectDialog = useAppStore((s) => s.openDeleteProjectDialog);

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
            {/* 契約 §7.1: 押した瞬間には消さない。確認ダイアログを開くだけ。 */}
            <button
              type="button"
              className="project-bar__delete"
              aria-label={`${p.name} を削除`}
              onClick={() => void openDeleteProjectDialog(p.id)}
            >
              ×
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

      {/* 削除ダイアログは ProjectBar の中にマウントする（契約 §130.3）。App.tsx の
          --z-scrim の層に並ぶ 2 枚（ProjectSwitcherContainer / SessionFormModal）の
          tree order は動かさない（src/App.projectSwitcher.test.tsx が固定している）。 */}
      <DeleteProjectDialogContainer />
    </div>
  );
}
