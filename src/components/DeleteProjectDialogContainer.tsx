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
 * 🔴 母数は `deleteProjectDialog.sessions`（opener が `list_sessions` で取ったもの）であって
 * `st.sessions` ではない。`st.sessions` は開いたことのあるプロジェクトの分しか持たないので、
 * そこから引くと未訪問プロジェクトの削除で常に「0 件」と出る（契約 §130.4 / §7.1）。
 *
 * 件数のためにセッション辞書全体を select しないこと —— 無関係なセッションが 1 つ更新
 * されるたびにダイアログが再レンダリングされる。セレクタはプリミティブを返す形にする。
 * 消える id の一覧は確定時にしか要らないので購読せず、`getState()` で読む。
 */
export function DeleteProjectDialogContainer(): JSX.Element | null {
  const projectId = useAppStore((s) => s.deleteProjectDialog?.projectId ?? null);
  const projectName = useAppStore((s) => s.projects.find((p) => p.id === projectId)?.name ?? null);
  const sessionCount = useAppStore((s) => s.deleteProjectDialog?.sessions?.length ?? null);
  const liveCount = useAppStore((s) => {
    const sessions = s.deleteProjectDialog?.sessions;
    if (sessions === undefined || sessions === null) return null;
    return sessions.filter((x) => isLive(s.runtimeStates[x.id])).length;
  });
  const error = useAppStore((s) => s.deleteProjectDialog?.error ?? null);
  const closeDeleteProjectDialog = useAppStore((s) => s.closeDeleteProjectDialog);
  const removeProject = useAppStore((s) => s.removeProject);
  const setError = useAppStore((s) => s.setError);

  if (projectId === null || projectName === null) return null;

  return (
    <DeleteProjectDialog
      projectName={projectName}
      sessionCount={sessionCount}
      liveCount={liveCount}
      error={error}
      onConfirm={() => {
        // 止める対象は数えたのと同じリストである（契約 §130.4 の「全セッションへ回す」）。
        const sessions = useAppStore.getState().deleteProjectDialog?.sessions;
        if (sessions === undefined || sessions === null) return;
        const sessionIds = sessions.map((x) => x.id);
        closeDeleteProjectDialog();
        // 失敗は契約 §6 の AppError のままトーストへ流す（§130.6 の日本語ラベルが付く）。
        removeProject(projectId, sessionIds).catch((e: unknown) => setError(toAppError(e)));
      }}
      onCancel={closeDeleteProjectDialog}
    />
  );
}
