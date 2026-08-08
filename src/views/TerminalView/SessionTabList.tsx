import { useShallow } from 'zustand/react/shallow';

import { RuntimeBadge } from '../../components/RuntimeBadge';
import { useAppStore } from '../../store';
import { paneBadgeFor } from '../../store/paneLogic';
import { selectTerminalTabs } from '../../store/terminalSlice';

// 契約 §29.7: M3-4 でここが 2 グループ（SESSIONS / SCRATCH）に分かれる。
// 正典クラスは kamux-tablist__group / kamux-tablist__group-label。
// is_scratch は schema_version 3 で入るため M1-3 では参照しないこと。
// 個別タブを別ファイルに切り出さない §25.2 の規則は維持すること。
//
// components.md「セッションタブ」節: 縦 2 段。1 段目 kamux-tab__title、
// 2 段目は左に runtime-badge + kamux-tab__cli、右に kamux-tab__pane-badge。
// kamux-tab__pane-badge は M3-2 の所有（単一ペインでは情報を運ばないため作らない。
// §57.5 と同じ理由）。kamux-tab__meta は kamux-tab の登録済みブロックから
// BEM で一意に決まる子要素なので個別登録は不要（§53.9.3 / §54.3 と同じ扱い）。
// M2-1 は kamux-tab__meta の中に <RuntimeBadge sessionId={id} /> を
// kamux-tab__cli の前に差し込む（data-session-id が目印）。
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

  return (
    <div className="kamux-tablist" role="tablist" aria-label="セッション">
      {tabs.map((id) => {
        // 契約 §28.3: split2 なら L / R、split2-v なら U / D、single なら null。
        // 「どちらがアクティブか」は aria-selected が表す（バッジは位置だけを表す）
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
            onClick={() => assignPane(activePane, id)}
          >
            <span className="kamux-tab__title">{sessions[id]?.title ?? id}</span>
            <span className="kamux-tab__meta">
              {/* runtimeStates を購読するのはこのバッジの中だけ（契約 §25.5 / §38.3）。
                タブ列がバッジの変化で再レンダリングされないよう props で渡さない */}
              <RuntimeBadge sessionId={id} />
              <span className="kamux-tab__cli" aria-hidden>
                {sessions[id]?.cli_kind}
              </span>
              {paneBadge !== null ? (
                <span className="kamux-tab__pane-badge">{paneBadge}</span>
              ) : null}
            </span>
          </button>
        );
      })}
    </div>
  );
}
