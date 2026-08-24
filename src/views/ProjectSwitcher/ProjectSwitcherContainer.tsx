import { useAppStore } from '../../store';
import { ProjectSwitcher } from './ProjectSwitcher';

/**
 * ストアと ProjectSwitcher の接続点。App.tsx に置き、SessionFormModal と兄弟になる
 * （マウント順の正典は src/App.projectSwitcher.test.tsx が固定している）。
 */
export function ProjectSwitcherContainer() {
  const open = useAppStore((s) => s.projectSwitcherOpen);
  const projects = useAppStore((s) => s.projects);
  const activeProjectId = useAppStore((s) => s.activeProjectId);
  const setProjectSwitcherOpen = useAppStore((s) => s.setProjectSwitcherOpen);
  const setActiveProject = useAppStore((s) => s.setActiveProject);

  if (!open) return null;

  return (
    <ProjectSwitcher
      projects={projects}
      activeProjectId={activeProjectId}
      onSelect={(id) => {
        setProjectSwitcherOpen(false);
        // PTY には触れない。表示を切り替えるだけ（設計書 §3 / §6.1）
        void setActiveProject(id);
      }}
      onClose={() => setProjectSwitcherOpen(false)}
    />
  );
}
