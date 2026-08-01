import { describe, expect, it } from 'vitest';
import { BRANCH_PREFIX, SLUG_MAX_LENGTH, proposeBranchName, titleToSlug } from './branchName';

describe('titleToSlug', () => {
  it('小文字化し、英数字とハイフン以外をハイフンにする', () => {
    expect(titleToSlug('Fix Login Bug')).toBe('fix-login-bug');
  });

  it('連続したハイフンを 1 つに圧縮する', () => {
    expect(titleToSlug('Fix   Login!!! Bug')).toBe('fix-login-bug');
  });

  it('前後のハイフンを除去する', () => {
    expect(titleToSlug('  --Fix Login--  ')).toBe('fix-login');
  });

  it('既存のハイフンは保つ', () => {
    expect(titleToSlug('re-run tests')).toBe('re-run-tests');
  });

  it('40 文字に切り詰める', () => {
    expect(titleToSlug('a'.repeat(50))).toHaveLength(SLUG_MAX_LENGTH);
  });

  it('切り詰めで残った末尾ハイフンも除去する', () => {
    // 40 文字の 'b' + スペース + 'c' → 41 文字目がハイフンになる
    expect(titleToSlug(`${'b'.repeat(40)} c`)).toBe('b'.repeat(40));
  });

  it('英数字を含まない入力では空文字を返す', () => {
    expect(titleToSlug('日本語タイトル')).toBe('');
    expect(titleToSlug('!!!')).toBe('');
    expect(titleToSlug('')).toBe('');
  });
});

describe('proposeBranchName', () => {
  it('契約 §13 のプレフィックスを付けて返す', () => {
    expect(BRANCH_PREFIX).toBe('session/');
    expect(proposeBranchName('Fix Login Bug')).toBe('session/fix-login-bug');
  });

  it('slug が空になる場合は null を返す（id を持つ Rust 側の fallback に委ねる）', () => {
    expect(proposeBranchName('日本語タイトル')).toBeNull();
    expect(proposeBranchName('')).toBeNull();
  });
});
