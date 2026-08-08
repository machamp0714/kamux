import { useEffect } from 'react';

import { useAppStore } from '../../store';
import { getTerminal } from '../../terminal/registry';
import { surfaceId } from '../../types/model';
import { SessionTabList } from './SessionTabList';
import { TerminalGrid } from './TerminalGrid';
import './TerminalView.css';

export function TerminalView(): JSX.Element {
  const focusedSessionId = useAppStore((state) => state.focusedSessionId);
  const view = useAppStore((state) => state.view);
  const modal = useAppStore((state) => state.modal);

  // 要件5: カードクリック / focus:// イベント（M2-3）の着地点。xterm インスタンスは
  // Zustand の外（registry）にあるため、DOM フォーカスはここで引いて当てる（契約 §10 / §16）。
  // モーダル表示中は実シェルへ DOM フォーカスを渡さない（契約 §16 の modal === null 規則）。
  //
  // **この effect（契約 §85.6 の「層 2」）は Task 9 が useActivePaneFocus へ統合して消す。**
  // それより前に消してはならない（契約 §85.5.1）——split2 では paneAssignment が動くので
  // 発火し、single では発火しないことがある間欠バグになる。
  useEffect(() => {
    if (view !== 'terminal' || focusedSessionId === null || modal !== null) return;
    const term = getTerminal(surfaceId(focusedSessionId, 'agent'));
    // DOM への mount 完了後にフォーカスするため 1 フレーム待つ
    const id = requestAnimationFrame(() => term?.focus());
    return () => cancelAnimationFrame(id);
  }, [focusedSessionId, view, modal]);

  return (
    <div className="kamux-terminal-view">
      <SessionTabList />
      <TerminalGrid />
    </div>
  );
}
