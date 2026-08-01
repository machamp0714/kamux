import { useEffect } from 'react';

import { useAppStore } from '../store';
import { resolveKeymap } from './keymap';

/**
 * keydown 1 件を resolveKeymap で判定し、ヒットした分だけ store に反映する。
 * useKeymap から window リスナとして直接張れるよう named export にしてある
 * （テストは window に KeyboardEvent を dispatch して検証する）。
 */
export function handleKeymapKeyDown(event: KeyboardEvent): void {
  const store = useAppStore.getState();
  const action = resolveKeymap(
    { key: event.key, metaKey: event.metaKey },
    { modalOpen: store.modal !== null },
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
