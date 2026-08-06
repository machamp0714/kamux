import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// React 18 の act() は既定でこのフラグを見て、テスト環境かどうかを判定する
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const mocks = vi.hoisted(() => ({
  spawnEditor: vi.fn(),
  resizePty: vi.fn(),
  onPtyExit: vi.fn(),
  ensurePtySubscription: vi.fn(),
  disposePtySubscription: vi.fn(),
  ensureTerminal: vi.fn(),
  attachTerminal: vi.fn(),
  detachTerminal: vi.fn(),
  disposeTerminal: vi.fn(),
  fitTerminal: vi.fn(),
  getTerminal: vi.fn(),
}));

vi.mock('../../ipc/commands', () => ({
  spawnEditor: mocks.spawnEditor,
  resizePty: mocks.resizePty,
}));
vi.mock('../../ipc/events', () => ({ onPtyExit: mocks.onPtyExit }));
vi.mock('../../terminal/ptyBridge', () => ({
  ensurePtySubscription: mocks.ensurePtySubscription,
  disposePtySubscription: mocks.disposePtySubscription,
}));
vi.mock('../../terminal/registry', () => ({
  ensureTerminal: mocks.ensureTerminal,
  attachTerminal: mocks.attachTerminal,
  detachTerminal: mocks.detachTerminal,
  disposeTerminal: mocks.disposeTerminal,
  fitTerminal: mocks.fitTerminal,
  getTerminal: mocks.getTerminal,
}));

import { useAppStore } from '../../store';
import type { Session } from '../../types/model';
import { EditorView } from './index';

function session(id: string, title: string): Session {
  return {
    id,
    project_id: 'p1',
    title,
    description: '',
    kanban_status: 'in_progress',
    sort_order: 1,
    mode: 'in_place',
    branch: null,
    worktree_path: null,
    cli_kind: 'shell',
    cli_command: null,
    claude_session_id: null,
    last_runtime_state: 'idle',
    last_runtime_error: null,
    first_started_at: null,
    archived_at: null,
    created_at: 0,
    updated_at: 0,
  };
}

class FakeResizeObserver {
  observe(): void {}
  disconnect(): void {}
}

async function flush(times = 6): Promise<void> {
  await act(async () => {
    for (let i = 0; i < times; i++) {
      await new Promise<void>((resolve) => queueMicrotask(() => resolve()));
    }
  });
}

let container: HTMLDivElement;
let root: Root;

async function render(): Promise<void> {
  act(() => {
    root = createRoot(container);
    root.render(<EditorView />);
  });
  await flush();
}

/** 文言でボタンを引く（クラス名ではなく利用者から見える形で選ぶ）。 */
function buttonByText(text: string): HTMLButtonElement {
  const found = Array.from(container.querySelectorAll('button')).find((b) =>
    (b.textContent ?? '').includes(text),
  );
  if (!found) throw new Error(`button not found: ${text}`);
  return found;
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal('ResizeObserver', FakeResizeObserver);
  mocks.ensureTerminal.mockImplementation(() => ({ options: {} }));
  mocks.onPtyExit.mockResolvedValue(() => {});
  mocks.ensurePtySubscription.mockResolvedValue(undefined);
  mocks.spawnEditor.mockResolvedValue('s1:editor');
  mocks.fitTerminal.mockReturnValue(null);
  mocks.getTerminal.mockReturnValue(undefined);
  useAppStore.setState({
    sessions: { s1: session('s1', 'セッション 1'), s2: session('s2', 'セッション 2') },
    modal: null,
    view: 'editor',
    focusedSessionId: 's1',
    editorSurfaces: {},
  });
  container = document.createElement('div');
  document.body.appendChild(container);
});

afterEach(() => {
  act(() => {
    root.unmount();
  });
  container.remove();
  vi.unstubAllGlobals();
});

describe('EditorView（空状態: セッション未選択では起動しない）', () => {
  it('focusedSessionId が null なら案内だけを出し、spawn_editor を投げない', async () => {
    useAppStore.setState({ focusedSessionId: null });

    await render();

    expect(container.querySelector('.editor-empty')).not.toBeNull();
    expect(container.querySelector('.editor-surface')).toBeNull();
    expect(mocks.spawnEditor).not.toHaveBeenCalled();
  });
});

describe('EditorView（D4: 終了したら再起動できる）', () => {
  it('exited なら再起動オーバーレイに exit code を出す', async () => {
    useAppStore.setState({ editorSurfaces: { s1: { kind: 'exited', exitCode: 3 } } });

    await render();

    const overlay = container.querySelector('.editor-overlay');
    expect(overlay).not.toBeNull();
    expect(overlay?.textContent).toContain('3');
  });

  it('再起動は disposeTerminal と disposePtySubscription を対で呼び、状態を捨ててから再起動する', async () => {
    useAppStore.setState({ editorSurfaces: { s1: { kind: 'exited', exitCode: 0 } } });

    await render();
    // 起動済み扱いなので、この時点では spawn し直していない
    expect(mocks.spawnEditor).not.toHaveBeenCalled();

    act(() => {
      buttonByText('再起動').click();
    });
    await flush();

    // 契約 §16: 片方だけだと再起動のたびに pty://data の購読が積み上がり、
    // 同じチャンクが N 回書き込まれて文字が二重に出る
    expect(mocks.disposeTerminal).toHaveBeenCalledWith('s1:editor');
    expect(mocks.disposePtySubscription).toHaveBeenCalledWith('s1:editor');
    // 状態を捨てたので EditorSurface が作り直され、遅延起動がもう一度走る
    expect(mocks.spawnEditor).toHaveBeenCalledWith('s1');
    expect(useAppStore.getState().editorSurfaces['s1']).toEqual({ kind: 'live' });
  });
});

describe('EditorView（上限エラー: 枠を空ける導線を出す）', () => {
  beforeEach(() => {
    useAppStore.setState({
      editorSurfaces: {
        s1: {
          kind: 'error',
          message:
            'editor limit reached: at most 3 nvim instances can be open at once. run :qa in one of them to free a slot',
        },
        s2: { kind: 'live' },
      },
    });
  });

  it('開いているエディタのセッション名を並べ、クリックでそのセッションの editor へ移る', async () => {
    await render();

    const overlay = container.querySelector('.editor-overlay');
    expect(overlay?.textContent).toContain(':qa');

    act(() => {
      buttonByText('セッション 2').click();
    });

    expect(useAppStore.getState().focusedSessionId).toBe('s2');
    expect(useAppStore.getState().view).toBe('editor');
  });

  it('上限エラーでは生の message を貼らない（案内文で置き換える）', async () => {
    await render();

    expect(container.textContent).not.toContain('editor limit reached');
  });
});

describe('EditorView（上限以外のエラー: 原文と再試行）', () => {
  it('message を原文のまま出し、再試行で状態を捨てて起動し直す', async () => {
    useAppStore.setState({
      editorSurfaces: {
        s1: { kind: 'error', message: '`nvim` が見つかりませんでした。' },
      },
    });

    await render();

    expect(container.querySelector('.editor-overlay')?.textContent).toContain(
      '`nvim` が見つかりませんでした。',
    );

    act(() => {
      buttonByText('再試行').click();
    });
    await flush();

    expect(mocks.disposeTerminal).toHaveBeenCalledWith('s1:editor');
    expect(mocks.disposePtySubscription).toHaveBeenCalledWith('s1:editor');
    expect(mocks.spawnEditor).toHaveBeenCalledWith('s1');
  });
});
