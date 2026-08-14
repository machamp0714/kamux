import { resumeAffordance } from '../../session/resumeAffordance';
import { useAppStore } from '../../store';

/**
 * PTY を持たないセッションのペインに重ねる想定のオーバーレイ（第1部 §4.4）。カードの
 * 再開ボタン（`KanbanCardResume`）と同じ `store.resumeSession` / `store.retryResumeAsFresh`
 * を呼ぶ（第1部 §4.4: 経路を分けない）。
 *
 * **この PR ではまだどこにもマウントされていない。** マウントには `TerminalPane` の
 * 無条件 `startSession`（契約 §85.5 条件 1）を条件付きにする必要があり、Task 10 の
 * 射程外である（実測: `grep -rln 'InterruptedOverlay' src --include='*.tsx'` は本ファイル・
 * そのテスト・`KanbanCardResume.tsx`（コメント内の言及のみ）の 3 ファイルで、
 * `TerminalGrid.tsx` / `TerminalPane.tsx` からの import・使用は 0 件）。
 */
export function InterruptedOverlay({ sessionId }: { sessionId: string }): JSX.Element | null {
  const session = useAppStore((s) => s.sessions[sessionId]);
  const failed = useAppStore((s) => s.resumeFailedSessionIds.includes(sessionId));
  const resume = useAppStore((s) => s.resumeSession);
  const retryFresh = useAppStore((s) => s.retryResumeAsFresh);

  if (!session) return null;
  const { label, note, warn } = resumeAffordance(session);

  return (
    <div className="interrupted-overlay">
      <p className="interrupted-overlay__title">このセッションは中断されています</p>
      {failed ? (
        <>
          <p className="interrupted-overlay__error">会話履歴が見つかりませんでした。</p>
          <button type="button" onClick={() => void retryFresh(sessionId)}>
            新しい会話として開始
          </button>
        </>
      ) : (
        <>
          <button type="button" onClick={() => void resume(sessionId)}>
            {label}
          </button>
          {note !== null && (
            <p className={warn ? 'interrupted-overlay__warn' : 'interrupted-overlay__note'}>
              {warn ? `⚠ ${note}` : note}
            </p>
          )}
        </>
      )}
    </div>
  );
}
