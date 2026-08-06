import { useCallback, useState } from 'react';

import { useAppStore } from '../../store';
import { disposePtySubscription } from '../../terminal/ptyBridge';
import { disposeTerminal } from '../../terminal/registry';
import { surfaceId } from '../../types/model';
import { EditorSurface } from './EditorSurface';
import { deriveEditorViewState, isEditorLimitError } from './editorViewState';
import './EditorView.css';

export function EditorView(): JSX.Element {
  const focusedSessionId = useAppStore((s) => s.focusedSessionId);
  const editorSurfaces = useAppStore((s) => s.editorSurfaces);
  const sessions = useAppStore((s) => s.sessions);
  const setEditorSurface = useAppStore((s) => s.setEditorSurface);
  const focusSession = useAppStore((s) => s.focusSession);

  // 再起動時に key を変えて EditorSurface を作り直す
  const [generation, setGeneration] = useState(0);

  const restart = useCallback(() => {
    if (focusedSessionId === null) return;
    const sid = surfaceId(focusedSessionId, 'editor');
    // PTY 終了後（または起動に失敗した後）にのみ到達する経路なので、
    // 契約 §16 の dispose 条件を満たす。
    // ★ disposeTerminal と disposePtySubscription は必ず対で呼ぶ（契約 §16）。
    //   片方を忘れると再起動のたびに pty://data の購読が 1 本ずつ積み上がり、
    //   同じチャンクが N 回書き込まれて文字が二重に出る。
    //   本ビューで disposeTerminal を呼ぶのはこの 1 箇所だけである
    //   （セッション切替は EditorSurface の detachTerminal → attachTerminal）
    disposeTerminal(sid);
    disposePtySubscription(sid);
    setEditorSurface(focusedSessionId, null);
    setGeneration((g) => g + 1);
  }, [focusedSessionId, setEditorSurface]);

  const state = deriveEditorViewState(
    focusedSessionId,
    focusedSessionId === null ? undefined : editorSurfaces[focusedSessionId],
  );

  if (state.kind === 'no_session' || focusedSessionId === null) {
    return (
      <div className="editor-empty">
        <p>セッションが選択されていません。</p>
        <p>
          <kbd>Cmd</kbd>+<kbd>1</kbd> でカンバンに戻り、セッションを選んでください。
        </p>
      </div>
    );
  }

  const openEditorSessionIds = Object.keys(editorSurfaces).filter(
    (id) => editorSurfaces[id]?.kind === 'live',
  );

  return (
    <div className="kamux-editor-view">
      <EditorSurface key={`${focusedSessionId}:${generation}`} sessionId={focusedSessionId} />

      {state.kind === 'exited' && (
        <div className="editor-overlay" role="alert">
          <p>nvim が終了しました（exit code {state.exitCode ?? '不明'}）。</p>
          <button type="button" autoFocus onClick={restart}>
            再起動
          </button>
        </div>
      )}

      {state.kind === 'error' && isEditorLimitError(state.message) && (
        <div className="editor-overlay" role="alert">
          <p>
            エディタは同時に 3 つまで開けます。どれかで <code>:qa</code> して枠を空けてください。
          </p>
          <ul className="editor-overlay__list">
            {openEditorSessionIds.map((id) => (
              <li key={id}>
                <button type="button" onClick={() => focusSession(id, 'editor')}>
                  {sessions[id]?.title ?? id}
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}

      {state.kind === 'error' && !isEditorLimitError(state.message) && (
        <div className="editor-overlay" role="alert">
          <p>nvim を起動できませんでした。</p>
          <pre className="editor-overlay__detail">{state.message}</pre>
          <button type="button" autoFocus onClick={restart}>
            再試行
          </button>
        </div>
      )}
    </div>
  );
}
