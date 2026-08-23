import { useEffect } from 'react';

import { setVisibilityContext } from '../ipc/commands';
import { useAppStore } from '../store';
import { visibleSessionIds } from '../store/visibility';

/**
 * 表示中のビューとセッションを Rust に push する。
 * Rust 側はこれを使って「見ているセッションの通知」を抑制する（設計 §5.4）。
 */
export function useVisibilityContext(): void {
  const view = useAppStore((s) => s.view);
  const layout = useAppStore((s) => s.layout);
  const focusedSessionId = useAppStore((s) => s.focusedSessionId);
  const paneAssignment = useAppStore((s) => s.paneAssignment.join(','));

  useEffect(() => {
    const panes = paneAssignment.split(',').map((v) => (v === '' ? null : v)) as [
      string | null,
      string | null,
    ];
    const ids = visibleSessionIds({ view, layout, focusedSessionId, paneAssignment: panes });
    void setVisibilityContext(view, ids);
  }, [view, layout, focusedSessionId, paneAssignment]);
}
