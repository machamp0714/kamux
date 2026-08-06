import { describe, expect, it } from 'vitest';
import type { Terminal } from '@xterm/xterm';
import { createTerminalKeyEventHandler, routeKeyEvent, type KeyRoutingInput } from './keyRouting';

const key = (over: Partial<KeyRoutingInput>): KeyRoutingInput => ({
  type: 'keydown',
  key: 'a',
  metaKey: false,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  ...over,
});

describe('routeKeyEvent', () => {
  it('nvim が使うキーはターミナルへ渡す', () => {
    expect(routeKeyEvent(key({ key: 'Escape' }))).toBe('terminal');
    expect(routeKeyEvent(key({ key: 'w', ctrlKey: true }))).toBe('terminal');
    expect(routeKeyEvent(key({ key: 'r', ctrlKey: true }))).toBe('terminal');
    expect(routeKeyEvent(key({ key: 'o', ctrlKey: true }))).toBe('terminal');
    expect(routeKeyEvent(key({ key: 'Tab' }))).toBe('terminal');
    expect(routeKeyEvent(key({ key: 'j', altKey: true }))).toBe('terminal');
    expect(routeKeyEvent(key({ key: ':' }))).toBe('terminal');
  });

  it('Cmd 系はアプリへ譲る', () => {
    expect(routeKeyEvent(key({ key: '3', metaKey: true }))).toBe('app');
    expect(routeKeyEvent(key({ key: '1', metaKey: true }))).toBe('app');
    expect(routeKeyEvent(key({ key: 'j', metaKey: true }))).toBe('app');
  });

  it('Cmd+C / Cmd+V もアプリへ譲る（ブラウザ既定のコピー/ペーストを走らせるため）', () => {
    expect(routeKeyEvent(key({ key: 'c', metaKey: true }))).toBe('app');
    expect(routeKeyEvent(key({ key: 'v', metaKey: true }))).toBe('app');
    expect(routeKeyEvent(key({ key: 'x', metaKey: true }))).toBe('app');
    expect(routeKeyEvent(key({ key: 'a', metaKey: true }))).toBe('app');
  });

  it('keyup / keypress でも同じ規則で分岐する', () => {
    expect(routeKeyEvent(key({ type: 'keyup', key: 'Escape' }))).toBe('terminal');
    expect(routeKeyEvent(key({ type: 'keypress', key: '3', metaKey: true }))).toBe('app');
  });
});

describe('createTerminalKeyEventHandler', () => {
  // 契約 §65.6 I1〜I4 を factory 単位で固定する。偽 term で書いてよい（§65.6）。
  // 実物の Terminal に `_keyDownSeen` が在ることの検査（T4）は
  // src/terminal/xtermCanary.test.ts の役目であり、ここには置かない。
  const fakeTerm = () => ({ _core: { _keyDownSeen: true } }) as unknown as Terminal;

  it('返り値の意味は routeKeyEvent と同じ（契約 §65.6 I3）', () => {
    const handler = createTerminalKeyEventHandler(fakeTerm());
    expect(handler(new KeyboardEvent('keydown', { key: 'Escape' }))).toBe(true);
    expect(handler(new KeyboardEvent('keydown', { key: '3', metaKey: true }))).toBe(false);
  });

  it('修飾キーのみの keydown で _keyDownSeen を false へ戻す（契約 §65.6 I1 / T1）', () => {
    const term = fakeTerm();
    const handler = createTerminalKeyEventHandler(term);
    expect(handler(new KeyboardEvent('keydown', { key: 'Shift' }))).toBe(true);
    expect((term as unknown as { _core: { _keyDownSeen: boolean } })._core._keyDownSeen).toBe(
      false,
    );
  });

  it('素のキーの keydown では触らない（契約 §65.6 I2 / T2）', () => {
    const term = fakeTerm();
    createTerminalKeyEventHandler(term)(new KeyboardEvent('keydown', { key: 'a' }));
    expect((term as unknown as { _core: { _keyDownSeen: boolean } })._core._keyDownSeen).toBe(true);
  });

  // 判別するのは event.type である。`if (event.type !== 'keydown') return;`（I4 のガード）を
  // 落とすと、修飾キーは MODIFIER_ONLY_KEYS を通るのでこの 2 本が赤くなる。
  // 名前が keyup / keypress の両方を主張しているので、両方を踏むこと。
  it.each(['keyup', 'keypress'])('%s では触らない（契約 §65.6 I4）', (type) => {
    const term = fakeTerm();
    createTerminalKeyEventHandler(term)(new KeyboardEvent(type, { key: 'Shift' }));
    expect((term as unknown as { _core: { _keyDownSeen: boolean } })._core._keyDownSeen).toBe(true);
  });
});
