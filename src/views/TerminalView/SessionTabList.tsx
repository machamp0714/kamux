import { useShallow } from 'zustand/react/shallow';

import { useAppStore } from '../../store';
import { selectTerminalTabs } from '../../store/terminalSlice';

// 契約 §29.7: M3-4 でここが 2 グループ（SESSIONS / SCRATCH）に分かれる。
// 正典クラスは kamux-tablist__group / kamux-tablist__group-label。
// is_scratch は schema_version 3 で入るため M1-3 では参照しないこと。
// 個別タブを別ファイルに切り出さない §25.2 の規則は維持すること。
// M2-1 がここに <RuntimeBadge sessionId={id} /> を差し込む（data-session-id が目印）。
export function SessionTabList(): JSX.Element {
  // 配列を返すセレクタなので useShallow で無駄な再レンダリングを防ぐ
  const tabs = useAppStore(useShallow(selectTerminalTabs));
  const sessions = useAppStore((state) => state.sessions);
  const activePane = useAppStore((state) => state.activePane);
  const current = useAppStore((state) => state.paneAssignment[state.activePane]);
  const assignPane = useAppStore((state) => state.assignPane);

  return (
    <div className="kamux-tablist" role="tablist" aria-label="セッション">
      {tabs.map((id) => (
        <button
          key={id}
          type="button"
          role="tab"
          className="kamux-tab"
          data-session-id={id}
          aria-selected={id === current}
          onClick={() => assignPane(activePane, id)}
        >
          {sessions[id]?.title ?? id} <span aria-hidden>[{sessions[id]?.cli_kind}]</span>
        </button>
      ))}
    </div>
  );
}
