import { describe, expect, it } from 'vitest';

import { deriveEditorViewState, isEditorLimitError } from './editorViewState';

describe('deriveEditorViewState', () => {
  it('セッション未選択なら no_session（spawn しない）', () => {
    expect(deriveEditorViewState(null, undefined)).toEqual({ kind: 'no_session' });
    expect(deriveEditorViewState(null, { kind: 'live' })).toEqual({ kind: 'no_session' });
  });

  it('状態が未登録なら starting（これから遅延起動する）', () => {
    expect(deriveEditorViewState('s1', undefined)).toEqual({ kind: 'starting' });
  });

  it('spawning も starting として扱う', () => {
    expect(deriveEditorViewState('s1', { kind: 'spawning' })).toEqual({ kind: 'starting' });
  });

  it('live をそのまま通す', () => {
    expect(deriveEditorViewState('s1', { kind: 'live' })).toEqual({ kind: 'live' });
  });

  it('exited は exit code を保持する', () => {
    expect(deriveEditorViewState('s1', { kind: 'exited', exitCode: 0 })).toEqual({
      kind: 'exited',
      exitCode: 0,
    });
    expect(deriveEditorViewState('s1', { kind: 'exited', exitCode: null })).toEqual({
      kind: 'exited',
      exitCode: null,
    });
  });

  it('error はメッセージを保持する', () => {
    expect(deriveEditorViewState('s1', { kind: 'error', message: 'boom' })).toEqual({
      kind: 'error',
      message: 'boom',
    });
  });
});

describe('isEditorLimitError', () => {
  it('spawn_editor の上限エラーを見分ける', () => {
    expect(
      isEditorLimitError(
        'editor limit reached: at most 3 nvim instances can be open at once. run :qa in one of them to free a slot',
      ),
    ).toBe(true);
  });

  it('他のエラーは false', () => {
    expect(isEditorLimitError('nvim not found in the login shell PATH (/usr/bin)')).toBe(false);
    expect(isEditorLimitError('work tree does not exist: /repo/.worktrees/x')).toBe(false);
    expect(isEditorLimitError('')).toBe(false);
  });
});
