import { useAppStore } from '../../store';
import { CleanupWorktreeDialog } from './CleanupWorktreeDialog';

/**
 * ストアと純表示コンポーネントをつなぐだけの薄い層。
 * 契約 §38.3 の許可リストにより、このファイルは `runtimeStates` を購読してよい
 * （逐語:「バッジを描かない購読者。稼働中警告の分岐に使う（M3-4 Task 9）」）。
 * モーダルは同時に 1 個しか存在せず、バッジを描かないので §25.5 の不変条件に触れない。
 */
export function CleanupWorktreeDialogContainer(): JSX.Element | null {
  const dialog = useAppStore((s) => s.cleanupDialog);
  const session = useAppStore((s) => (dialog ? s.sessions[dialog.sessionId] : undefined));
  // undefined のまま渡す。session.last_runtime_state で埋めない（契約 §38.3 論点 2）
  const runtimeState = useAppStore((s) => (dialog ? s.runtimeStates[dialog.sessionId] : undefined));
  const closeCleanupDialog = useAppStore((s) => s.closeCleanupDialog);
  const confirmCleanup = useAppStore((s) => s.confirmCleanup);
  const focusSession = useAppStore((s) => s.focusSession);

  if (!dialog || !session) return null;

  return (
    <CleanupWorktreeDialog
      worktreePath={session.worktree_path ?? ''}
      branch={session.branch}
      runtimeState={runtimeState}
      status={dialog.status}
      error={dialog.error}
      busy={dialog.busy}
      onConfirm={(force) => void confirmCleanup(force)}
      onCancel={closeCleanupDialog}
      onOpenTerminal={() => {
        closeCleanupDialog();
        focusSession(session.id, 'terminal');
      }}
    />
  );
}
