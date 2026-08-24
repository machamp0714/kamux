import type { MouseEvent, PointerEvent } from 'react';

import { useAppStore } from '../../store';
import { isCleanupSuggested } from '../../store/cleanup';

/** ボタン押下がドラッグ開始に化けないようにする（KanbanCard.tsx の stopDrag と同じ形。第1部 判断 7）。 */
function stopDrag(e: PointerEvent<HTMLButtonElement>) {
  e.stopPropagation();
}

/** クリックがカードの onOpen へバブリングしないようにする（KanbanCard.tsx の stopOpen と同じ形）。 */
function stopOpen(e: MouseEvent<HTMLButtonElement>) {
  e.stopPropagation();
}

/**
 * 済んだセッションの worktree を掃除する導線（M3-4 Task 9）。
 *
 * **葉が自分で購読する形**にしてある（`KanbanCardResume` と同じ流儀）。`KanbanCard` は
 * 「ストアも dnd-kit の hook も自分では触らないので、DragOverlay のクローンでもそのまま
 * 使える」（`KanbanCard.tsx` の doc）と決まっており、そこでストアを触るとこの設計が壊れる。
 *
 * 提案は押し付けない —— `isCleanupSuggested()`（`store/cleanup.ts`。M3-4 Task 8）が true の
 * ときに控えめなボタンを出すだけで、ダイアログを自動では開かない。
 */
export function KanbanCardCleanup({ sessionId }: { sessionId: string }): JSX.Element | null {
  const session = useAppStore((s) => s.sessions[sessionId]);
  const openCleanupDialog = useAppStore((s) => s.openCleanupDialog);

  if (!session) return null;
  if (!isCleanupSuggested(session)) return null;

  return (
    <button
      type="button"
      title="worktree を掃除"
      aria-label="worktree を掃除"
      onPointerDown={stopDrag}
      onClick={(e) => {
        stopOpen(e);
        void openCleanupDialog(sessionId);
      }}
    >
      🧹 worktree
    </button>
  );
}
