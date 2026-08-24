import { useAppStore } from '../store';
import { toAppError } from '../store/uiSlice';
import type { RuntimeState } from '../types/model';
import { DeleteProjectDialog } from './DeleteProjectDialog';

/**
 * 「稼働中」の 2 値。`CleanupWorktreeDialog.tsx` の `live` と同じ判定に揃えてある
 * （逐語: `runtimeState === 'running' || runtimeState === 'waiting_input'`）。
 * `undefined`（実行状態が未知 = 未起動 / 最初の `session://state` 到着前）は数えない ——
 * 知らないときに「動いています」とは言わない（契約 §38.3 論点 2 / §33.3 Q1）。
 */
function isLive(state: RuntimeState | undefined): boolean {
  return state === 'running' || state === 'waiting_input';
}

/**
 * ストアと純表示コンポーネントをつなぐだけの薄い層。マウント先は `ProjectBar`
 * （契約 §130.3: 速度のための `ProjectSwitcher` に破壊操作を混ぜない）。
 *
 * 契約 §38.3 の許可リストにより、このファイルは `runtimeStates` を購読してよい
 * （逐語:「バッジを描かない購読者。**削除確認ダイアログの稼働中件数の表示に使う**」）。
 * 🔴 購読者はこのファイルだけである —— `DeleteProjectDialog.tsx` も `ProjectBar.tsx` も
 * 購読しない。両方が購読すると許可リストの行が 2 つ要り、それは移設ではなく追加になる
 * （契約 §147.4）。射程は表示だけで、`stop_session` の分岐には使わない（契約 §147.2）。
 *
 * 件数のためにセッション辞書全体を select しないこと —— 無関係なセッションが 1 つ更新
 * されるたびにダイアログが再レンダリングされる。セレクタはプリミティブを返す形にする。
 */
export function DeleteProjectDialogContainer(): JSX.Element | null {
  const projectId = useAppStore((s) => s.deleteProjectDialog?.projectId ?? null);
  const projectName = useAppStore((s) => s.projects.find((p) => p.id === projectId)?.name ?? null);
  const sessionCount = useAppStore((s) =>
    projectId === null
      ? 0
      : Object.values(s.sessions).filter((x) => x.project_id === projectId).length,
  );
  const liveCount = useAppStore((s) =>
    projectId === null
      ? 0
      : Object.values(s.sessions).filter(
          (x) => x.project_id === projectId && isLive(s.runtimeStates[x.id]),
        ).length,
  );
  const closeDeleteProjectDialog = useAppStore((s) => s.closeDeleteProjectDialog);
  const removeProject = useAppStore((s) => s.removeProject);
  const setError = useAppStore((s) => s.setError);

  if (projectId === null || projectName === null) return null;

  return (
    <DeleteProjectDialog
      projectName={projectName}
      sessionCount={sessionCount}
      liveCount={liveCount}
      onConfirm={() => {
        closeDeleteProjectDialog();
        // 失敗は契約 §6 の AppError のままトーストへ流す（§130.6 の日本語ラベルが付く）。
        removeProject(projectId).catch((e: unknown) => setError(toAppError(e)));
      }}
      onCancel={closeDeleteProjectDialog}
    />
  );
}
