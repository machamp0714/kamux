import { useEffect } from 'react';

import { useAppStore } from '../../store';
import { getTerminal } from '../../terminal/registry';
import { surfaceId } from '../../types/model';
import { SessionTabList } from './SessionTabList';
import { TerminalPane } from './TerminalPane';
import './TerminalView.css';

export function TerminalView(): JSX.Element {
  // M3-2 で split2 / split2-v に対応する（契約 §28）。M1-3 は activePane の 1 面のみ
  const sessionId = useAppStore((state) => state.paneAssignment[state.activePane]);
  const focusedSessionId = useAppStore((state) => state.focusedSessionId);
  const view = useAppStore((state) => state.view);
  const modal = useAppStore((state) => state.modal);

  // 要件5: カードクリック / focus:// イベント（M2-3）の着地点。xterm インスタンスは
  // Zustand の外（registry）にあるため、DOM フォーカスはここで引いて当てる（契約 §10 / §16）。
  // モーダル表示中は実シェルへ DOM フォーカスを渡さない（契約 §16 の modal === null 規則）。
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
      <TerminalPane sessionId={sessionId} />
    </div>
  );
}
