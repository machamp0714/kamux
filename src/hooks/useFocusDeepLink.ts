import { useEffect } from 'react';
import type { UnlistenFn } from '@tauri-apps/api/event';

import { listenFocus } from '../ipc/events';
import { useAppStore } from '../store';
import { selectSessionIdsKey } from '../store/sessionSlice';

/**
 * 通知クリック起点の `focus://session/{session_id}` を購読し、該当セッションへ飛ぶ。
 *
 * トピックに session_id が埋まっている（契約 §8）ため、ストア上のセッションごとに
 * リスナを張る。セッションが増減したら張り直す。
 *
 * App.tsx にインラインで実装されていた同種の effect（M1-4）をこのフックへ抽出した
 * ものであり、新規追加ではない（Ruling AE）。
 */
export function useFocusDeepLink(): void {
  const sessionIdsKey = useAppStore(selectSessionIdsKey);
  const focusSession = useAppStore((s) => s.focusSession);

  useEffect(() => {
    const ids = sessionIdsKey ? sessionIdsKey.split(',') : [];
    const unlistens: Promise<UnlistenFn>[] = ids.map((id) =>
      listenFocus(id, (p) =>
        focusSession(p.session_id, p.surface_kind === 'editor' ? 'editor' : 'terminal'),
      ),
    );
    return () => {
      unlistens.forEach((u) => void u.then((fn) => fn()));
    };
  }, [sessionIdsKey, focusSession]);
}
