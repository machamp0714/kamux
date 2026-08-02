import { describe, expect, it } from 'vitest';

import { canSubmitProjectForm } from './ProjectBar';

describe('canSubmitProjectForm', () => {
  it('名前とリポジトリパスが両方非空なら送信できる', () => {
    expect(canSubmitProjectForm({ name: 'kamux', repoPath: '/repo/kamux' })).toBe(true);
  });

  it('名前が空なら送信できない', () => {
    expect(canSubmitProjectForm({ name: '', repoPath: '/repo/kamux' })).toBe(false);
  });

  it('リポジトリパスが空なら送信できない', () => {
    expect(canSubmitProjectForm({ name: 'kamux', repoPath: '' })).toBe(false);
  });

  it('空白のみの入力は非空として扱わない', () => {
    expect(canSubmitProjectForm({ name: '  ', repoPath: '/repo/kamux' })).toBe(false);
    expect(canSubmitProjectForm({ name: 'kamux', repoPath: '   ' })).toBe(false);
  });
});
