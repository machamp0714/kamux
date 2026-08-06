import { describe, expect, it, vi } from 'vitest';
import { applyEditorTerminalOptions, type EditorTerminalLike } from './editorTerminalOptions';

// 偽 term は EditorTerminalLike より広い。attachCustomKeyEventHandler を「持っている」
// のに applyEditorTerminalOptions が呼ばないことを示すため、意図的に生やしてある
// （契約 §65.6 I3 / §77）。EditorTerminalLike 自体は options しか持たない。
const fakeTerminal = (): EditorTerminalLike & {
  options: { macOptionIsMeta?: boolean; scrollback?: number };
  handler: ((e: KeyboardEvent) => boolean) | null;
  attachCustomKeyEventHandler(h: (e: KeyboardEvent) => boolean): void;
} => ({
  options: {},
  handler: null,
  attachCustomKeyEventHandler(h) {
    this.handler = h;
  },
});

describe('applyEditorTerminalOptions', () => {
  it('nvim の <M-x> マッピングを効かせるため macOptionIsMeta を立てる', () => {
    const term = fakeTerminal();
    applyEditorTerminalOptions(term);
    expect(term.options.macOptionIsMeta).toBe(true);
  });

  it('scrollback には触れない（M1-3 の ensureTerminal が :editor を見て 1,000 行にする）', () => {
    const term = fakeTerminal();
    applyEditorTerminalOptions(term);
    expect(term.options.scrollback).toBeUndefined();
  });

  // 契約 §65.6 I3 / §77: 設定点は ensureTerminal の 1 箇所である。ここで再 attach すると
  // registry.ts が設定した副作用付きハンドラを純粋な述語で奪い、nvim のインサートモードで
  // `!` / `@` / Shift+英字の 1 打目が消える。
  it('attachCustomKeyEventHandler を呼ばない（registry.ts のハンドラを奪わない）', () => {
    const term = fakeTerminal();
    const spy = vi.spyOn(term, 'attachCustomKeyEventHandler');
    applyEditorTerminalOptions(term);
    expect(spy).not.toHaveBeenCalled();
    expect(term.handler).toBeNull();
  });

  it('二重適用しても同じ結果になる（冪等）', () => {
    const term = fakeTerminal();
    const spy = vi.spyOn(term, 'attachCustomKeyEventHandler');
    applyEditorTerminalOptions(term);
    applyEditorTerminalOptions(term);
    expect(term.options.macOptionIsMeta).toBe(true);
    expect(term.options.scrollback).toBeUndefined();
    expect(spy).toHaveBeenCalledTimes(0);
  });
});
