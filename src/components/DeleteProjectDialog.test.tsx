import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';

import { DeleteProjectDialog } from './DeleteProjectDialog';

afterEach(cleanup);

const props = () => ({
  projectName: 'kamux',
  sessionCount: 3,
  liveCount: 1,
  onConfirm: vi.fn(),
  onCancel: vi.fn(),
});

describe('DeleteProjectDialog', () => {
  it('消す対象のプロジェクト名を出す（契約 §130.4）', () => {
    render(<DeleteProjectDialog {...props()} />);
    expect(screen.getByRole('dialog', { name: 'プロジェクトを削除' })).toBeTruthy();
    expect(screen.getByText('kamux')).toBeTruthy();
  });

  it('消えるセッション数と稼働中の件数を別々に出す（契約 §130.4）', () => {
    render(<DeleteProjectDialog {...props()} />);
    // 3 と 1 は取り違えたら別物になる具体値。同じ素の number が 2 本ある。
    expect(screen.getByText('セッション 3 件が一緒に消えます')).toBeTruthy();
    expect(screen.getByText('うち稼働中 1 件')).toBeTruthy();
  });

  it('作業ツリーは残ることを 1 行出す（契約 §130.4。消す導線は 🧹 が持つ）', () => {
    render(<DeleteProjectDialog {...props()} />);
    expect(screen.getByText(/作業ツリーは残ります/)).toBeTruthy();
  });

  it('稼働中が 0 件なら稼働中の行を出さない（ダイアログ本体は描かれている）', () => {
    render(<DeleteProjectDialog {...props()} liveCount={0} />);
    // 「出ない」だけを見ると、画面が丸ごと描かれていなくても通る。他が描かれていることを添える。
    expect(screen.getByText('セッション 3 件が一緒に消えます')).toBeTruthy();
    expect(screen.queryByText(/うち稼働中/)).toBeNull();
  });

  // components.md「モーダル・ダイアログ」節:
  // 不可逆操作は明示的なチェックボックスを通す。チェックが入るまで実行ボタンは無効。
  it('チェックを入れるまで削除ボタンは無効（components.md）', () => {
    const p = props();
    render(<DeleteProjectDialog {...p} />);

    const button = screen.getByRole('button', { name: '削除する' }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    fireEvent.click(button);
    expect(p.onConfirm).not.toHaveBeenCalled();
  });

  it('チェックを入れてから削除ボタンで onConfirm、キャンセルで onCancel を呼ぶ', () => {
    const p = props();
    render(<DeleteProjectDialog {...p} />);

    fireEvent.click(screen.getByRole('checkbox'));
    fireEvent.click(screen.getByRole('button', { name: '削除する' }));
    expect(p.onConfirm).toHaveBeenCalledTimes(1);
    expect(p.onCancel).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'キャンセル' }));
    expect(p.onCancel).toHaveBeenCalledTimes(1);
    expect(p.onConfirm).toHaveBeenCalledTimes(1);
  });

  it('スクリムを押すと onCancel を呼ぶ', () => {
    const p = props();
    const { container } = render(<DeleteProjectDialog {...p} />);

    const backdrop = container.querySelector('.delete-project-dialog__backdrop');
    expect(backdrop).not.toBeNull();
    fireEvent.mouseDown(backdrop as Element);
    expect(p.onCancel).toHaveBeenCalledTimes(1);
  });

  it('ダイアログ本体を押しても閉じない（スクリムへ伝播させない）', () => {
    const p = props();
    const { container } = render(<DeleteProjectDialog {...p} />);

    fireEvent.mouseDown(container.querySelector('.delete-project-dialog') as Element);
    expect(p.onCancel).not.toHaveBeenCalled();
    // 「呼ばれない」だけを見ると要素が無くても通る。要素が在ることを添える。
    expect(screen.getByRole('dialog', { name: 'プロジェクトを削除' })).toBeTruthy();
  });

  // CleanupWorktreeDialog.tsx の doc と同じ判断（外側を無効化していないので宣言しない）。
  it('aria-modal を宣言しない（CleanupWorktreeDialog と同じ判断）', () => {
    render(<DeleteProjectDialog {...props()} />);
    expect(screen.getByRole('dialog').getAttribute('aria-modal')).toBeNull();
  });
});
