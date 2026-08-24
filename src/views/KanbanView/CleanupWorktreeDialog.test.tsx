import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { CleanupWorktreeDialog } from './CleanupWorktreeDialog';

// vite.config.ts の test に globals の設定が無いため、@testing-library/react の
// 自動 cleanup は登録されない（globals: true のときだけ afterEach が張られる）。
// 明示しないと render した DOM が積み上がり、getByRole が複数マッチで落ちる。
afterEach(cleanup);

const base = {
  worktreePath: '/repo/a/.worktrees/session-fix-login',
  branch: 'session/fix-login',
  runtimeState: 'idle' as const,
  busy: false,
  error: null,
  onConfirm: vi.fn(),
  onCancel: vi.fn(),
  onOpenTerminal: vi.fn(),
};

describe('CleanupWorktreeDialog', () => {
  it('取得中はスピナー文言を出し、削除ボタンを無効にする', () => {
    render(<CleanupWorktreeDialog {...base} status={null} />);
    expect(screen.getByText('変更を確認しています…')).toBeTruthy();
    expect(screen.getByRole('button', { name: '削除する' })).toHaveProperty('disabled', true);
  });

  it('clean なら「ブランチは残ります」を出し、そのまま削除できる', () => {
    const onConfirm = vi.fn();
    render(
      <CleanupWorktreeDialog
        {...base}
        onConfirm={onConfirm}
        status={{ dirty: false, entries: [] }}
      />,
    );

    // ブランチ名は <code> の中にあるため、既定のマッチャ（要素直下のテキストノードだけを
    // 連結する getNodeText）では <p> にも <code> にも一致しない。文言で <p> を引き当て、
    // ブランチ名は textContent 全体を見る toHaveTextContent で確かめる。
    const note = screen.getByText(/は残ります/);
    expect(note).toHaveTextContent('ブランチ session/fix-login は残ります');

    expect(screen.queryByLabelText('変更を破棄して強制削除する')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: '削除する' }));

    expect(onConfirm).toHaveBeenCalledWith(false);
  });

  it('dirty なら未コミット一覧を出し、チェックするまで削除できない', () => {
    const onConfirm = vi.fn();
    render(
      <CleanupWorktreeDialog
        {...base}
        onConfirm={onConfirm}
        status={{ dirty: true, entries: ['?? new.txt', ' M README.md'] }}
      />,
    );

    expect(screen.getByText('未コミットの変更があります')).toBeTruthy();
    expect(screen.getByText('?? new.txt')).toBeTruthy();
    // 先頭のスペースは `git status --porcelain` の意味の一部（' M' = unstaged な変更）。
    // 既定の normalizer は要素側のテキストを trim するが matcher 側の文字列は trim しないため
    // 素の getByText(' M README.md') は一致しない。normalizer を無効化して原文で引く
    // （トリム版で引くと、実装が先頭スペースを落としても緑になる）。
    expect(screen.getByText(' M README.md', { normalizer: (s) => s })).toBeTruthy();

    const button = screen.getByRole('button', { name: '強制削除する' });
    expect(button).toHaveProperty('disabled', true);
    fireEvent.click(button);
    expect(onConfirm).not.toHaveBeenCalled();

    fireEvent.click(screen.getByLabelText('変更を破棄して強制削除する'));
    expect(screen.getByRole('button', { name: '強制削除する' })).toHaveProperty('disabled', false);
    fireEvent.click(screen.getByRole('button', { name: '強制削除する' }));

    expect(onConfirm).toHaveBeenCalledWith(true);
  });

  it('dirty のときだけ「ターミナルで確認する」を出す', () => {
    const onOpenTerminal = vi.fn();
    const { rerender } = render(
      <CleanupWorktreeDialog
        {...base}
        onOpenTerminal={onOpenTerminal}
        status={{ dirty: true, entries: ['?? a'] }}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'ターミナルで確認する' }));
    expect(onOpenTerminal).toHaveBeenCalled();

    rerender(
      <CleanupWorktreeDialog
        {...base}
        onOpenTerminal={onOpenTerminal}
        status={{ dirty: false, entries: [] }}
      />,
    );
    expect(screen.queryByRole('button', { name: 'ターミナルで確認する' })).toBeNull();
  });

  it('セッションが動いている間は警告を出すが、削除自体は止めない', () => {
    const onConfirm = vi.fn();
    render(
      <CleanupWorktreeDialog
        {...base}
        onConfirm={onConfirm}
        runtimeState="running"
        status={{ dirty: false, entries: [] }}
      />,
    );

    expect(screen.getByText(/このセッションはまだ動いています/)).toBeTruthy();
    const button = screen.getByRole('button', { name: '削除する' });
    expect(button).toHaveProperty('disabled', false);
    fireEvent.click(button);
    expect(onConfirm).toHaveBeenCalledWith(false);
  });

  it('waiting_input でも稼働中の警告を出す', () => {
    render(
      <CleanupWorktreeDialog
        {...base}
        runtimeState="waiting_input"
        status={{ dirty: false, entries: [] }}
      />,
    );
    expect(screen.getByText(/このセッションはまだ動いています/)).toBeTruthy();
  });

  it('停止済みなら警告を出さない', () => {
    render(
      <CleanupWorktreeDialog
        {...base}
        runtimeState="exited"
        status={{ dirty: false, entries: [] }}
      />,
    );
    expect(screen.queryByText(/このセッションはまだ動いています/)).toBeNull();
  });

  it('実行状態が未知（undefined）なら警告を出さない', () => {
    // container は runtimeStates[id] が undefined のとき session.last_runtime_state で
    // 埋めない（契約 §38.3 論点 2）。知らない状態について何も主張しないこと
    render(
      <CleanupWorktreeDialog
        {...base}
        runtimeState={undefined}
        status={{ dirty: false, entries: [] }}
      />,
    );
    expect(screen.queryByText(/このセッションはまだ動いています/)).toBeNull();
  });

  it('error は加工せずそのまま表示する', () => {
    render(
      <CleanupWorktreeDialog
        {...base}
        status={{ dirty: true, entries: [] }}
        error={"fatal: '/x' contains modified or untracked files, use --force to delete it\n"}
      />,
    );
    expect(screen.getByText(/use --force to delete it/)).toBeTruthy();
  });

  it('キャンセルで onCancel が呼ばれる', () => {
    const onCancel = vi.fn();
    render(
      <CleanupWorktreeDialog
        {...base}
        onCancel={onCancel}
        status={{ dirty: false, entries: [] }}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'キャンセル' }));
    expect(onCancel).toHaveBeenCalled();
  });

  it('busy の間は削除ボタンを無効にする（二重送信の防止）', () => {
    render(<CleanupWorktreeDialog {...base} busy status={{ dirty: false, entries: [] }} />);
    expect(screen.getByRole('button', { name: '削除する' })).toHaveProperty('disabled', true);
  });

  it('status が差し替わったら破棄チェックを引き継がない', () => {
    const { rerender } = render(
      <CleanupWorktreeDialog {...base} status={{ dirty: true, entries: ['?? a'] }} />,
    );
    fireEvent.click(screen.getByLabelText('変更を破棄して強制削除する'));
    expect(screen.getByRole('button', { name: '強制削除する' })).toHaveProperty('disabled', false);

    // 取り直した status（別オブジェクト）に差し替わったら、前のチェックは無効になる
    rerender(
      <CleanupWorktreeDialog {...base} status={{ dirty: true, entries: ['?? a', '?? b'] }} />,
    );

    expect(screen.getByLabelText('変更を破棄して強制削除する')).toHaveProperty('checked', false);
    expect(screen.getByRole('button', { name: '強制削除する' })).toHaveProperty('disabled', true);
  });
});

// 裁定 46 で aria-modal を外した結果、閉じる手段はキャンセルボタンとスクリムだけになる
// （Escape は cleanupDialog が uiSlice の modal とは別フィールドなので keymap.ts:78 に乗らない）。
// SessionFormModal.tsx:119 と同じ形（backdrop の onMouseDown で閉じ、パネル側で止める）を固定する。
describe('CleanupWorktreeDialog のスクリム（SessionFormModal と同じ形）', () => {
  it('スクリムの mouseDown で onCancel が呼ばれる', () => {
    const onCancel = vi.fn();
    const { container } = render(
      <CleanupWorktreeDialog
        {...base}
        onCancel={onCancel}
        status={{ dirty: false, entries: [] }}
      />,
    );

    const backdrop = container.querySelector('.cleanup-worktree-dialog__backdrop');
    expect(backdrop).not.toBeNull();
    fireEvent.mouseDown(backdrop as Element);

    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it('パネル内の mouseDown では onCancel が呼ばれない', () => {
    const onCancel = vi.fn();
    render(
      <CleanupWorktreeDialog
        {...base}
        onCancel={onCancel}
        status={{ dirty: false, entries: [] }}
      />,
    );

    fireEvent.mouseDown(screen.getByRole('dialog', { name: 'worktree を掃除' }));

    expect(onCancel).not.toHaveBeenCalled();
  });

  it('aria-modal を宣言しない（外側を無効化していないため。kanban-view__drawer と同じ判断）', () => {
    render(<CleanupWorktreeDialog {...base} status={{ dirty: false, entries: [] }} />);
    expect(screen.getByRole('dialog', { name: 'worktree を掃除' })).not.toHaveAttribute(
      'aria-modal',
    );
  });
});
