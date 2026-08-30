import { useShallow } from 'zustand/react/shallow';

import { RuntimeBadge } from '../../components/RuntimeBadge';
import { useAppStore } from '../../store';
import { paneBadgeFor, type PaneIndex, type PaneState } from '../../store/paneLogic';
import { selectTerminalTabs } from '../../store/terminalSlice';
import type { Session } from '../../types/model';

// 契約 §29.7: タブ列は SESSIONS / SCRATCH の 2 グループに分けて描く。
// 振り分けは !is_scratch → SESSIONS / is_scratch → SCRATCH（描画側の分類。
// 順序を決める selectTerminalTabs 自体は変えない）。正典クラスは
// kamux-tablist__group / kamux-tablist__group-label。空グループは見出しごと
// 描かない（SCRATCH が 0 件のときに空の見出しだけ残るのを避ける）。role="tablist"
// は親 div 1 つのまま（グループの div は表示上のまとまりであり a11y ツリーで
// tablist を割らない）。個別タブを別ファイルに切り出さない §25.2 の規則は維持する。
//
// components.md「セッションタブ」節: 縦 2 段。1 段目 kamux-tab__title、
// 2 段目は左に runtime-badge + kamux-tab__cli、右に kamux-tab__pane-badge。
// kamux-tab__pane-badge は M3-2 の所有(単一ペインでは情報を運ばないため作らない。
// §57.5 と同じ理由)。kamux-tab__meta は kamux-tab の登録済みブロックから
// BEM で一意に決まる子要素なので個別登録は不要(§53.9.3 / §54.3 と同じ扱い)。
// M2-1 は kamux-tab__meta の中に <RuntimeBadge sessionId={id} /> を
// kamux-tab__cli の前に差し込む(data-session-id が目印)。

function renderTab(
  id: string,
  sessions: Record<string, Session>,
  paneState: PaneState,
  current: string | null,
  assignPane: (pane: PaneIndex, sessionId: string) => void,
): JSX.Element {
  // 契約 §28.3: split2 なら L / R、split2-v なら U / D、single なら null。
  // 「どちらがアクティブか」は aria-selected が表す(バッジは位置だけを表す)
  const paneBadge = paneBadgeFor(paneState, id);
  return (
    <button
      key={id}
      type="button"
      role="tab"
      className="kamux-tab"
      data-session-id={id}
      data-pane-badge={paneBadge ?? ''}
      aria-selected={id === current}
      onClick={() => assignPane(paneState.activePane, id)}
    >
      <span className="kamux-tab__title">{sessions[id]?.title ?? id}</span>
      <span className="kamux-tab__meta">
        {/* runtimeStates を購読するのはこのバッジの中だけ(契約 §25.5 / §38.3)。
          タブ列がバッジの変化で再レンダリングされないよう props で渡さない */}
        <RuntimeBadge sessionId={id} />
        <span className="kamux-tab__cli" aria-hidden>
          {sessions[id]?.cli_kind}
        </span>
        {paneBadge !== null ? <span className="kamux-tab__pane-badge">{paneBadge}</span> : null}
      </span>
    </button>
  );
}

export function SessionTabList(): JSX.Element {
  // 配列を返すセレクタなので useShallow で無駄な再レンダリングを防ぐ
  const tabs = useAppStore(useShallow(selectTerminalTabs));
  const sessions = useAppStore((state) => state.sessions);
  const layout = useAppStore((state) => state.layout);
  const paneAssignment = useAppStore((state) => state.paneAssignment);
  const activePane = useAppStore((state) => state.activePane);
  const current = useAppStore((state) => state.paneAssignment[state.activePane]);
  const assignPane = useAppStore((state) => state.assignPane);

  const paneState = { layout, paneAssignment, activePane };

  const sessionTabs = tabs.filter((id) => !sessions[id]?.is_scratch);
  const scratchTabs = tabs.filter((id) => sessions[id]?.is_scratch);

  return (
    <div className="kamux-tablist" role="tablist" aria-label="セッション">
      {sessionTabs.length > 0 ? (
        <div className="kamux-tablist__group" role="presentation">
          <span className="kamux-tablist__group-label" aria-hidden>
            SESSIONS
          </span>
          {sessionTabs.map((id) => renderTab(id, sessions, paneState, current, assignPane))}
        </div>
      ) : null}
      {scratchTabs.length > 0 ? (
        <div className="kamux-tablist__group" role="presentation">
          <span className="kamux-tablist__group-label" aria-hidden>
            SCRATCH
          </span>
          {scratchTabs.map((id) => renderTab(id, sessions, paneState, current, assignPane))}
        </div>
      ) : null}
    </div>
  );
}
