import { describe, expect, it } from 'vitest';
import { BRANCH_PREFIX, proposeBranchName, titleToSlug } from './branchName';

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
    // 契約 §13 の 40 をリテラルで固定する（SLUG_MAX_LENGTH の自己参照にしない）
    expect(titleToSlug('a'.repeat(50))).toHaveLength(40);
  });

  it('切り詰めで残った末尾ハイフンも除去する', () => {
    // 'a' を 39 個（index 0〜38）+ 空白（index 39 でハイフンに変換）+ 'zzzz'。
    // slice(0, 40) は index 0〜39 を含むため、ハイフンがちょうど切り詰め境界の
    // 末尾（40 文字目）に残る。この末尾ハイフンが .replace(/-+$/, '') で
    // 除去されることを確認する。
    expect(titleToSlug(`${'a'.repeat(39)} zzzz`)).toBe('a'.repeat(39));
  });

  it('英数字を含まない入力では空文字を返す', () => {
    expect(titleToSlug('日本語タイトル')).toBe('');
    expect(titleToSlug('!!!')).toBe('');
    expect(titleToSlug('')).toBe('');
  });

  it('絵文字のみでは空文字を返す', () => {
    expect(titleToSlug('🎉')).toBe('');
  });

  it('空白のみでは空文字を返す', () => {
    expect(titleToSlug('   ')).toBe('');
  });

  it('絵文字を含む混在入力では絵文字を除去する', () => {
    expect(titleToSlug('Fix 🎉 bug')).toBe('fix-bug');
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
