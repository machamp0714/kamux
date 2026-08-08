import { SessionTabList } from './SessionTabList';
import { TerminalGrid } from './TerminalGrid';
import './TerminalView.css';

// 要件5: カードクリック / focus:// イベント（M2-3）の着地点は
// `TerminalGrid` が呼ぶ `useActivePaneFocus`（terminal 面の唯一のフォーカス層。
// 契約 §85.3 / §85.5.1 / §85.6）に統合されている。ここでは何もしない。
export function TerminalView(): JSX.Element {
  return (
    <div className="kamux-terminal-view">
      <SessionTabList />
      <TerminalGrid />
    </div>
  );
}
