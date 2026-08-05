import { useEffect, useMemo, useRef } from 'react';
import type { UnlistenFn } from '@tauri-apps/api/event';

import { listenSessionState } from '../ipc/events';
import { useAppStore } from '../store';

type Handle = UnlistenFn | 'pending';

/**
 * `session://state/{session_id}` をセッション単位で購読する。
 *
 * Tauri 2 にワイルドカード購読がないためセッションごとに listen するが、
 * 一覧が変わるたびに全件張り直すと購読の解除/再登録の隙にイベントを落とす。
 * ソート済み ID 文字列をキーにして、増減分だけ listen / unlisten する。
 *
 * `listen()` は Promise を返すため、登録が完了する前に同じ ID の解除要求が
 * 来うる。その間は handles を 'pending' にしておき、解決時に「まだ要るか」を
 * 見て自分で unlisten するかどうかを判定する（取りこぼし防止）。
 */
export function useRuntimeStateEvents(sessionIds: string[]): void {
  const applyStateEvent = useAppStore((s) => s.applyStateEvent);
  const handles = useRef(new Map<string, Handle>());

  const key = useMemo(() => [...sessionIds].sort().join(','), [sessionIds]);

  useEffect(() => {
    const ids = key ? key.split(',') : [];
    const map = handles.current;

    for (const [id, handle] of [...map]) {
      if (ids.includes(id)) continue;
      if (handle !== 'pending') handle();
      // 'pending' の場合は削除だけしておく。listenSessionState の解決時に
      // 「もう地図に無い」ことを見て、そちら側で unlisten させる。
      map.delete(id);
    }

    for (const id of ids) {
      if (map.has(id)) continue;
      map.set(id, 'pending');
      void listenSessionState(id, applyStateEvent).then((unlisten) => {
        if (map.get(id) === 'pending') {
          map.set(id, unlisten);
        } else {
          // 登録完了前に解除要求が来ていた（map から既に削除済み）。
          // ここで unlisten しないと、削除されたはずのセッションが
          // イベントを受け続けるリークになる。
          unlisten();
        }
      });
    }
  }, [key, applyStateEvent]);

  useEffect(() => {
    const map = handles.current;
    return () => {
      for (const [, handle] of map) {
        if (handle !== 'pending') handle();
      }
      map.clear();
    };
  }, []);
}
