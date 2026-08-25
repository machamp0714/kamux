import { useState } from 'react';
import {
  DndContext,
  DragOverlay,
  KeyboardSensor,
  PointerSensor,
  closestCorners,
  useSensor,
  useSensors,
  type DragEndEvent,
  type DragStartEvent,
} from '@dnd-kit/core';
import { sortableKeyboardCoordinates } from '@dnd-kit/sortable';
import { useAppStore } from '../../store';
import { toAppError } from '../../store/uiSlice';
import { HooksStatusPanel } from '../../components/HooksStatusPanel';
import { KANBAN_STATUSES, type Session } from '../../types/model';
import { ArchivedDrawer } from './ArchivedDrawer';
import { CleanupWorktreeDialogContainer } from './CleanupWorktreeDialogContainer';
import { KanbanCard } from './KanbanCard';
import { KanbanColumn } from './KanbanColumn';
import { resolveDragEnd } from './dragEnd';
import { KANBAN_KEYBOARD_CODES, KANBAN_POINTER_ACTIVATION_DISTANCE } from './sensors';
import './kanban.css';

/**
 * HooksStatusPanel は全セッション横断のパネルなので、activeProjectId で絞らず
 * ストアの sessions を全件そのまま渡す（lane-controller の統合裁定）。
 */
function toSessionTitles(sessions: Record<string, Session>): Record<string, string> {
  const titles: Record<string, string> = {};
  for (const s of Object.values(sessions)) titles[s.id] = s.title;
  return titles;
}

export function KanbanView() {
  const sessions = useAppStore((s) => s.sessions);
  const sessionOrder = useAppStore((s) => s.sessionOrder);
  const moveCard = useAppStore((s) => s.moveCard);
  const openModal = useAppStore((s) => s.openModal);
  const setError = useAppStore((s) => s.setError);
  const activeProjectId = useAppStore((s) => s.activeProjectId);
  // アーカイブ済みドロワー（M3-4 Task 10）。網羅検査（paneInvariant.test.ts の
  // 「引数表 ARGS が母数（組み立て済みストアの関数型メンバ全体）を覆っている」テスト）の
  // 対象にするため uiSlice に置く。hooks 疎通ステータスのドロワー（hooksOpen）は
  // その検査と無関係なのでローカル state のままにする。
  const showArchived = useAppStore((s) => s.showArchived);
  const setShowArchived = useAppStore((s) => s.setShowArchived);
  const restoreSession = useAppStore((s) => s.restoreSession);
  const openCleanupDialog = useAppStore((s) => s.openCleanupDialog);
  const [draggingId, setDraggingId] = useState<string | null>(null);
  const [hooksOpen, setHooksOpen] = useState(false);

  const sensors = useSensors(
    useSensor(PointerSensor, {
      activationConstraint: { distance: KANBAN_POINTER_ACTIVATION_DISTANCE },
    }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
      keyboardCodes: KANBAN_KEYBOARD_CODES,
    }),
  );

  const onDragStart = (event: DragStartEvent) => setDraggingId(String(event.active.id));

  const onDragEnd = (event: DragEndEvent) => {
    setDraggingId(null);
    const activeId = String(event.active.id);
    const overId = event.over === null ? null : String(event.over.id);
    const result = resolveDragEnd(activeId, overId, sessionOrder);
    if (result === null) return;
    // moveCard は失敗時に巻き戻して rethrow する（M1-1 の契約）。
    // エラーの提示は呼び出し側の責務（第1部 判断 2）
    moveCard(activeId, result.to, result.index).catch((e: unknown) => setError(toAppError(e)));
  };

  const dragging = draggingId === null ? undefined : sessions[draggingId];

  return (
    <div className="kanban-view">
      <header className="kanban-view__header">
        <h1 className="kanban-view__heading">カンバン</h1>
        <div className="kanban-view__actions">
          <button type="button" className="kanban-view__hooks" onClick={() => setHooksOpen(true)}>
            hooks 疎通ステータス
          </button>
          <button
            type="button"
            className="kanban-view__archived"
            onClick={() => setShowArchived(true)}
          >
            アーカイブ済み
          </button>
          <button
            type="button"
            className="kanban-view__new"
            onClick={() => openModal({ kind: 'create_session' })}
          >
            新規セッション <kbd>⌘N</kbd>
          </button>
        </div>
      </header>

      {/* パネルは開いている間だけマウントする。マウント時に 1 回だけ取得する設計
          （HooksStatusPanel 参照）なので、開くたびに最新の診断が読まれる。 */}
      {hooksOpen ? (
        <div className="kanban-view__drawer-scrim" onMouseDown={() => setHooksOpen(false)}>
          {/* aria-modal は宣言しない。SessionFormModal（SessionFormModal.tsx:122-123）は
              keymap.ts:77 経由で Escape が効くが、このドロワーは開閉をローカル state
              （hooksOpen）に置いているため resolveKeymap の modalOpen 判定に乗らず、
              Escape で閉じる手段が無い。focus trap も無い（inert / tabIndex 管理は無し。
              背後の要素は依然 Tab で到達できる）。`aria-modal` は「外側は無効化されて
              いる」と宣言するが、本実装は外側を無効化していない（focus trap も
              `inert` も無い）。宣言と実態が食い違うと、支援技術の利用者は外側の内容を
              隠されたまま、`Escape` という慣習的な閉じ方も持たない（閉じる手段は
              `閉じる` ボタンとスクリムのみ）。宣言側を実態に合わせて外す。 */}
          <aside
            className="kanban-view__drawer"
            role="dialog"
            aria-label="hooks 疎通ステータス"
            onMouseDown={(e) => e.stopPropagation()}
          >
            <div className="kanban-view__drawer-header">
              <button
                type="button"
                className="kanban-view__drawer-close"
                onClick={() => setHooksOpen(false)}
              >
                閉じる
              </button>
            </div>
            <HooksStatusPanel sessionTitles={toSessionTitles(sessions)} />
          </aside>
        </div>
      ) : null}

      <DndContext
        sensors={sensors}
        collisionDetection={closestCorners}
        onDragStart={onDragStart}
        onDragEnd={onDragEnd}
        onDragCancel={() => setDraggingId(null)}
      >
        <div className="kanban-view__board">
          {KANBAN_STATUSES.map((status) => (
            <KanbanColumn
              key={status}
              status={status}
              // 二重防御（契約 §29.4）。主たる境界は buildSessionOrder と
              // move_session（session_dao.rs）。sessionOrder に scratch の id が
              // 紛れ込んでもここで落とす。sessions[id] は undefined になりうる
              // （move_session の戻り値を反映する経路が buildSessionOrder を経由しない
              // ため、sessionOrder にあって sessions に無い id が到達しうる）ので、
              // オプショナルチェーンで扱い、未知の場合は除外しない。
              sessionIds={sessionOrder[status].filter((id) => sessions[id]?.is_scratch !== true)}
              sessions={sessions}
            />
          ))}
        </div>
        <DragOverlay>
          {dragging === undefined ? null : <KanbanCard session={dragging} />}
        </DragOverlay>
      </DndContext>

      {/* open が false の間は自分で null を返す（M3-4 Task 10）。 */}
      <ArchivedDrawer
        open={showArchived}
        sessions={Object.values(sessions).filter((s) => s.project_id === activeProjectId)}
        onRestore={(id) => {
          restoreSession(id).catch((e: unknown) => setError(toAppError(e)));
        }}
        onCleanup={(id) => void openCleanupDialog(id)}
        onClose={() => setShowArchived(false)}
      />

      {/* cleanupDialog が null の間は自分で null を返す（M3-4 Task 9）。
          .cleanup-worktree-dialog__backdrop と .archived-drawer__scrim は同一の --z-scrim
          を使うため、同一スタッキングレベルでは tree order が重なり順を決める（CSS 仕様）。
          ArchivedDrawer より後にマウントすることで、確認ダイアログが常に ArchivedDrawer
          より前面へ来るようにする（PR #106 全体レビュー I-1。この重なり順は
          index.test.tsx で固定している。tokens.css は編集しない —— 契約 §53.2 が
          実装者によるトークンの追加・変更を禁じている）。
          KanbanView の外にある SessionFormModal との重なりはこの並び順では閉じない
          （本 PR の射程外。lane-controller が team-lead へ上げる）。 */}
      <CleanupWorktreeDialogContainer />
    </div>
  );
}
