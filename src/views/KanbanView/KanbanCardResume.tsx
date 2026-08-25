import type { MouseEvent, PointerEvent } from 'react';

import { resumeAffordance } from '../../session/resumeAffordance';
import { useAppStore } from '../../store';

/** ボタン押下がドラッグ開始に化けないようにする（KanbanCard.tsx の stopDrag と同じ形。第1部 判断 7）。 */
function stopDrag(e: PointerEvent<HTMLButtonElement>) {
  e.stopPropagation();
}

/** クリックがカードの onOpen へバブリングしないようにする（KanbanCard.tsx の stopOpen と同じ形）。 */
function stopOpen(e: MouseEvent<HTMLButtonElement>) {
  e.stopPropagation();
}

/**
 * 中断/終了したセッションの再開ボタン（第1部 §4.4）。
 *
 * **葉が自分で購読する形**にしてある（契約 §25.5 / §38.3。`KanbanCardError` と同じ流儀）。
 * `KanbanCard` の中で `runtimeStates` を購読すると、実行状態が動くたびカード全体が
 * 再レンダリングされ、契約 §25.5 の不変条件が壊れる。
 *
 * `interrupted` / `exited` 以外は何も描かない。`resumeFailedSessionIds`
 * （reason: 'resume_failed' を受け取ったセッション。契約 §8）に載っている
 * セッションは「新しい会話として開始」（`retryResumeAsFresh`）に切り替わる。
 * それ以外は `resumeAffordance()` のラベルで通常の再開（`resumeSession`）を出す。
 *
 * 再開の導線は `store.resumeSession` / `store.retryResumeAsFresh` のこの 1 経路だけを
 * 呼ぶ（第1部 §4.4: 経路を分けない）。
 */
export function KanbanCardResume({ sessionId }: { sessionId: string }): JSX.Element | null {
  const runtimeState = useAppStore((s) => s.runtimeStates[sessionId]);
  const session = useAppStore((s) => s.sessions[sessionId]);
  const failed = useAppStore((s) => s.resumeFailedSessionIds.includes(sessionId));
  const resumeSession = useAppStore((s) => s.resumeSession);
  const retryResumeAsFresh = useAppStore((s) => s.retryResumeAsFresh);

  if (runtimeState !== 'interrupted' && runtimeState !== 'exited') return null;
  if (!session) return null;

  if (failed) {
    return (
      <button
        type="button"
        onPointerDown={stopDrag}
        onClick={(e) => {
          stopOpen(e);
          void retryResumeAsFresh(sessionId);
        }}
      >
        新しい会話として開始
      </button>
    );
  }

  const { label, note } = resumeAffordance(session);
  return (
    <button
      type="button"
      title={note ?? undefined}
      onPointerDown={stopDrag}
      onClick={(e) => {
        stopOpen(e);
        void resumeSession(sessionId);
      }}
    >
      {label}
    </button>
  );
}
