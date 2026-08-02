import { useAppStore } from '../../store';
import { SessionTabList } from './SessionTabList';
import { TerminalPane } from './TerminalPane';
import './TerminalView.css';

export function TerminalView(): JSX.Element {
  // M3-2 で split2 / split2-v に対応する（契約 §28）。M1-3 は activePane の 1 面のみ
  const sessionId = useAppStore((state) => state.paneAssignment[state.activePane]);

  return (
    <div className="kamux-terminal-view">
      <SessionTabList />
      <TerminalPane sessionId={sessionId} />
    </div>
  );
}
