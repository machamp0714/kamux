import { useAppStore } from '../../store';

/**
 * ❌ のカードに生 stderr を原文で出す（契約 §42.4 / デザインシステムのカード節）。
 *
 * **`RuntimeBadge` と同じく葉が自分で購読する形**にしてある。`KanbanCard` の中で
 * `runtimeStates` を購読すると、バッジやエラーの変化がカード全体の再レンダリングへ
 * 波及して契約 §25.5 の不変条件が壊れる。購読の許可は契約 §38.3 の許可リストにある。
 *
 * `state !== 'error'` と `!message` の両方で null を返す —— ❌ だがメッセージがまだ
 * 無い一瞬（イベントが catch より先に着いた場合）に空枠を描かないため。
 *
 * タブ列（`SessionTabList`）には置かない（§42.4）。
 */
export function KanbanCardError({ sessionId }: { sessionId: string }): JSX.Element | null {
  const state = useAppStore((s) => s.runtimeStates[sessionId]);
  const message = useAppStore((s) => s.runtimeErrors[sessionId]);
  if (state !== 'error' || !message) return null;
  return <pre className="kanban-card__error">{message}</pre>;
}
