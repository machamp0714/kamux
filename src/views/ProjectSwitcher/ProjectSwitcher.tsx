import { useMemo, useState, type KeyboardEvent } from 'react';

import type { Project } from '../../types/model';
import { filterProjects } from './filterProjects';
import './ProjectSwitcher.css';

export interface ProjectSwitcherProps {
  projects: Project[];
  activeProjectId: string | null;
  onSelect: (id: string) => void;
  onClose: () => void;
}

/**
 * Cmd+P の絞り込みモーダル（契約 §11.4.2 の Cmd+P 行 / §49.7）。
 * 常時表示の ProjectBar とは別物であり共存する（置き換えない）。
 * 破壊操作（削除）はここに置かない —— この面は速度のためにある（契約 §130.3）。
 *
 * 絞り込みは filterProjects（Task 11）に委譲する。空クエリなら全件が名前順で返る。
 */
export function ProjectSwitcher({
  projects,
  activeProjectId,
  onSelect,
  onClose,
}: ProjectSwitcherProps) {
  const [query, setQuery] = useState('');
  const [cursor, setCursor] = useState(0);
  const matches = useMemo(() => filterProjects(projects, query), [projects, query]);
  // 候補数が減ってもカーソルが範囲外へ出ないようにクランプする（0 件なら 0）。
  const index = Math.min(cursor, Math.max(matches.length - 1, 0));

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
      return;
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setCursor((c) => Math.min(c + 1, matches.length - 1));
      return;
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      setCursor((c) => Math.max(c - 1, 0));
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      const picked = matches[index];
      if (picked) onSelect(picked.id);
    }
  };

  return (
    <div className="project-switcher__scrim">
      <div
        className="project-switcher"
        role="dialog"
        aria-modal="true"
        aria-label="プロジェクトを切り替え"
      >
        <input
          className="project-switcher__input"
          role="combobox"
          aria-expanded="true"
          aria-label="プロジェクト名またはパス"
          autoFocus
          value={query}
          placeholder="プロジェクト名またはパス"
          onChange={(e) => {
            setQuery(e.target.value);
            setCursor(0);
          }}
          onKeyDown={onKeyDown}
        />
        {matches.length === 0 ? (
          <p className="project-switcher__empty">該当するプロジェクトがありません</p>
        ) : (
          <ul className="project-switcher__list" role="listbox">
            {matches.map((p, i) => (
              <li
                key={p.id}
                className="project-switcher__item"
                role="option"
                aria-selected={i === index}
                data-active={p.id === activeProjectId}
                onMouseEnter={() => setCursor(i)}
                onClick={() => onSelect(p.id)}
              >
                <strong className="project-switcher__name">{p.name}</strong>
                <span className="project-switcher__path">{p.repo_path}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
