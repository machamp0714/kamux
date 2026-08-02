import { describe, expect, it } from 'vitest';
import { CLI_ICON, RUNTIME_BADGE, runtimeBadge } from './badge';

describe('RUNTIME_BADGE', () => {
  // 契約 §2 の RuntimeState は 6 値。toEqual は厳密比較なので、
  // 表に値を足したらこの期待値も同時に直すこと（契約 §38.4）
  it('契約 §2 の 6 状態すべてに設計書 §6.2 / 契約 §33.5 のグリフを持つ', () => {
    expect(RUNTIME_BADGE).toEqual({
      running: '🟢',
      waiting_input: '🟡',
      idle: '⚪',
      exited: '⛔',
      interrupted: '⏸',
      error: '❌',
    });
  });
});

describe('CLI_ICON', () => {
  it('契約 §2 の 4 種すべてにアイコンを持つ', () => {
    expect(Object.keys(CLI_ICON).sort()).toEqual(['claude', 'codex', 'custom', 'shell']);
  });
});

describe('runtimeBadge', () => {
  it('runtimeStates に値があればそれを使う', () => {
    expect(runtimeBadge({ s1: 'waiting_input' }, 's1')).toEqual({ glyph: '🟡', label: '入力待ち' });
  });

  it('interrupted を ⏸ で表す', () => {
    expect(runtimeBadge({ s1: 'interrupted' }, 's1')?.glyph).toBe('⏸');
  });

  it('exited を ⛔ で表す', () => {
    expect(runtimeBadge({ s1: 'exited' }, 's1')?.glyph).toBe('⛔');
  });

  it('error を ❌「エラー」で表す（契約 §33.5 のラベル正典）', () => {
    expect(runtimeBadge({ s1: 'error' }, 's1')).toEqual({ glyph: '❌', label: 'エラー' });
  });

  it('runtimeStates に値が無ければ null を返す（契約 §34.7）', () => {
    expect(runtimeBadge({}, 's1')).toBeNull();
  });

  it('last_runtime_state にはフォールバックしない（第1部 判断 6）', () => {
    // DB 側が running / waiting_input のまま残っていても、runtimeStates が空なら
    // 🟢 や 🟡 を出してはいけない。正規化（interrupted 付与）は M2-1 の
    // normalize_on_startup() の責務で、M1-2 の時点では走っていない。
    expect(runtimeBadge({}, 's1')).toBeNull();
    expect(runtimeBadge({ other: 'running' }, 's1')).toBeNull();
  });
});
