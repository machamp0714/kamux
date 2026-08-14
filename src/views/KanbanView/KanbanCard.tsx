import type { DraggableAttributes, DraggableSyntheticListeners } from '@dnd-kit/core';
import type { KeyboardEvent, MouseEvent, PointerEvent } from 'react';
import { RuntimeBadge } from '../../components/RuntimeBadge';
import type { Session } from '../../types/model';
import { CLI_ICON } from './badge';
import { KanbanCardError } from './KanbanCardError';
import { KanbanCardResume } from './KanbanCardResume';

/**
 * dnd-kit のドラッグ活性化一式。**ラッパ要素ではなくカードのルート要素に載せる。**
 * ラッパに載せるとカード 1 枚あたりのタブストップが 2 つになり、先頭のストップでは
 * `.kanban-card:focus-within`（kanban.css）が一致せずアクションが出ないまま
 * Enter も効かない（React の onKeyDown は祖先を target とするイベントを見られない）。
 */
export interface CardDragActivator {
  ref: (node: HTMLElement | null) => void;
  attributes: DraggableAttributes;
  listeners: DraggableSyntheticListeners;
}

export interface KanbanCardProps {
  session: Session;
  onEdit?: (sessionId: string) => void;
  onArchive?: (sessionId: string) => void;
  /** クリック / Enter でセッションを開く（要件5）。 */
  onOpen?: (sessionId: string) => void;
  dragActivator?: CardDragActivator;
}

/** ボタン押下がドラッグ開始に化けないようにする（第1部 判断 7）。 */
function stopDrag(e: PointerEvent<HTMLButtonElement>) {
  e.stopPropagation();
}

/**
 * カード内のボタン（編集 / アーカイブ）のクリックがカード自体の
 * onClick（= onOpen）へバブリングしないようにする（要件5）。
 */
function stopOpen(e: MouseEvent<HTMLButtonElement>) {
  e.stopPropagation();
}

/**
 * カードの見た目を持つ。ストアも dnd-kit の hook も自分では触らないので、
 * DragOverlay のクローンでもそのまま使える —— クローンには `onOpen` も
 * `dragActivator` も渡さないので、対話性（タブストップ・クリック・キー操作）を
 * 一切持たない見た目だけの複製になる。
 * クリック / Enter でターミナル画面の該当ペインへフォーカスする（要件5・契約 §11）。
 *
 * **`runtimeStates` / `runtimeErrors` を購読しない**（契約 §25.5 / §38.3）。
 * バッジ・エラー・再開ボタンは葉（`RuntimeBadge` / `KanbanCardError` / `KanbanCardResume`）
 * が自分で購読するので、実行状態が動いてもカード全体は再レンダリングされない。
 */
export function KanbanCard({ session, onEdit, onArchive, onOpen, dragActivator }: KanbanCardProps) {
  const dragListeners = dragActivator?.listeners;
  // 開ける、またはドラッグできるときだけタブストップにする
  const interactive = onOpen !== undefined || dragActivator !== undefined;

  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    // ドラッグ開始（Space・sensors.ts）は dnd-kit の onKeyDown が担う。
    // 下で onKeyDown を明示的に指定する以上、spread された同名ハンドラは
    // 上書きされるので、ここから自分で呼ぶ必要がある。
    dragListeners?.onKeyDown?.(e);
    if (onOpen === undefined || e.key !== 'Enter' || e.defaultPrevented) return;
    e.preventDefault();
    onOpen(session.id);
  };

  return (
    <article
      className="kanban-card"
      data-session-id={session.id}
      ref={dragActivator?.ref}
      {...dragActivator?.attributes}
      {...dragListeners}
      tabIndex={interactive ? 0 : undefined}
      onClick={onOpen === undefined ? undefined : () => onOpen(session.id)}
      onKeyDown={onKeyDown}
    >
      {/* components.md「カード」節: head は両端寄せの 2 グループ（左に CLI チップ、
          右にバッジ）。title は head の中ではなくカード直下の子要素である */}
      <div className="kanban-card__head">
        <span className="kanban-card__cli" title={session.cli_kind} aria-label={session.cli_kind}>
          {CLI_ICON[session.cli_kind]}
        </span>
        <span className="kanban-card__badge">
          <RuntimeBadge sessionId={session.id} />
        </span>
      </div>

      <h3 className="kanban-card__title">{session.title}</h3>

      {session.description !== '' ? (
        <p className="kanban-card__description">{session.description}</p>
      ) : null}

      {session.mode === 'worktree' && session.branch !== null ? (
        <p className="kanban-card__branch" title={session.branch}>
          {session.branch}
        </p>
      ) : null}

      <KanbanCardError sessionId={session.id} />

      <div className="kanban-card__actions">
        {interactive && <KanbanCardResume sessionId={session.id} />}
        <button
          type="button"
          onPointerDown={stopDrag}
          onClick={(e) => {
            stopOpen(e);
            onEdit?.(session.id);
          }}
        >
          編集
        </button>
        <button
          type="button"
          onPointerDown={stopDrag}
          onClick={(e) => {
            stopOpen(e);
            onArchive?.(session.id);
          }}
        >
          アーカイブ
        </button>
      </div>
    </article>
  );
}
