import { useEffect } from 'react';

import { useAppStore } from '../store';
import { resolveKeymap } from './keymap';

/**
 * IME 変換確定前の Escape（変換キャンセル）かどうかを DOM イベントから判定する。
 * isComposing が立たないブラウザ経路向けに keyCode 229 のレガシー判定も併用する
 * （OS 分岐ではなく、あくまで IME 変換中かどうかの判定）。
 */
function isImeCancelEscape(event: KeyboardEvent): boolean {
  return event.key === 'Escape' && (event.isComposing || event.keyCode === 229);
}

/**
 * keydown 1 件を resolveKeymap で判定し、ヒットした分だけ store に反映する。
 * useKeymap から window リスナとして直接張れるよう named export にしてある
 * （テストは window に KeyboardEvent を dispatch して検証する）。
 *
 * IME 変換中の Escape（変換キャンセル）はモーダルを閉じる Escape として扱わない。
 * この判定は DOM の KeyboardEvent が持つ isComposing / keyCode に依存するため、
 * 純関数の resolveKeymap ではなくここ（DOM 側）に置く。Cmd+N / Cmd+1 は
 * 変換中でも奪う（契約 §11）ので、ガードは Escape の分岐にだけ効かせる。
 */
export function handleKeymapKeyDown(event: KeyboardEvent): void {
  if (isImeCancelEscape(event)) return;
  const store = useAppStore.getState();
  const action = resolveKeymap(
    { key: event.key, metaKey: event.metaKey },
    { modalOpen: store.modal !== null, view: store.view },
  );
  if (action === null) return;
  event.preventDefault();
  switch (action.type) {
    case 'set_view':
      store.setView(action.view);
      break;
    case 'open_create_session':
      store.openModal({ kind: 'create_session' });
      break;
    case 'close_modal':
      store.closeModal();
      break;
    case 'cycle_session':
      store.cycleSession(action.dir);
      break;
  }
}

/**
 * window に単一の keydown リスナを張る（契約 §11）。
 * xterm やテキスト入力がフォーカスを持っていても Cmd 系は奪う。
 * ストアは useAppStore.getState() で読むので、依存配列は空でよい。
 */
export function useKeymap(): void {
  useEffect(() => {
    window.addEventListener('keydown', handleKeymapKeyDown);
    return () => window.removeEventListener('keydown', handleKeymapKeyDown);
  }, []);
}
