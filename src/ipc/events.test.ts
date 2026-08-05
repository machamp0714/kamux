import { beforeEach, describe, expect, it, vi } from 'vitest';

const listen = vi.fn();
vi.mock('@tauri-apps/api/event', () => ({
  listen: (...args: unknown[]) => listen(...args),
}));

import type { FocusPayload } from '../types/model';
import { listenFocus, onPtyData, onPtyExit } from './events';

beforeEach(() => {
  listen.mockReset();
});

describe('ipc/events', () => {
  it('onPtyData が契約どおりのトピック文字列を listen する', async () => {
    const unlisten = vi.fn();
    listen.mockResolvedValue(unlisten);
    const handler = vi.fn();

    const result = await onPtyData('s1:agent', handler);

    expect(listen).toHaveBeenCalledWith('pty://data/s1:agent', expect.any(Function));
    expect(result).toBe(unlisten);
  });

  it('onPtyData のハンドラは event.payload を渡す（event 全体ではない）', async () => {
    let capturedCallback: ((event: { payload: unknown }) => void) | undefined;
    listen.mockImplementation((_topic: string, callback: (event: { payload: unknown }) => void) => {
      capturedCallback = callback;
      return Promise.resolve(vi.fn());
    });
    const handler = vi.fn();

    await onPtyData('s1:agent', handler);
    const payload = { base64: 'AAEC', seq: 1 };
    capturedCallback?.({ payload });

    expect(handler).toHaveBeenCalledWith(payload);
  });

  it('onPtyExit が契約どおりのトピック文字列を listen する', async () => {
    const unlisten = vi.fn();
    listen.mockResolvedValue(unlisten);
    const handler = vi.fn();

    const result = await onPtyExit('s1:agent', handler);

    expect(listen).toHaveBeenCalledWith('pty://exit/s1:agent', expect.any(Function));
    expect(result).toBe(unlisten);
  });

  it('onPtyExit のハンドラに surface_id / exit_code（snake_case）を持つ payload が渡る', async () => {
    let capturedCallback: ((event: { payload: unknown }) => void) | undefined;
    listen.mockImplementation((_topic: string, callback: (event: { payload: unknown }) => void) => {
      capturedCallback = callback;
      return Promise.resolve(vi.fn());
    });

    let received: { surface_id: string; exit_code: number | null } | undefined;
    await onPtyExit('s1:agent', (payload) => {
      received = payload;
    });
    capturedCallback?.({ payload: { surface_id: 's1:agent', exit_code: null } });

    expect(received?.surface_id).toBe('s1:agent');
    expect(received?.exit_code).toBeNull();
  });

  // 契約 §35: listenFocus は focus:// トピックの listen ラッパの正典であり、
  // M2-3（通知クリック）は events.ts を変更せず import するだけである。
  // トピック文字列のタイプミスはここで止めないと無検証のまま M2-3 に載る。
  it('listenFocus が契約どおりのトピック文字列を listen する', async () => {
    const unlisten = vi.fn();
    listen.mockResolvedValue(unlisten);
    const handler = vi.fn();

    const result = await listenFocus('s1', handler);

    expect(listen).toHaveBeenCalledWith('focus://session/s1', expect.any(Function));
    expect(result).toBe(unlisten);
  });

  it('listenFocus のハンドラに session_id / surface_kind（snake_case）を持つ payload が渡る', async () => {
    let capturedCallback: ((event: { payload: unknown }) => void) | undefined;
    listen.mockImplementation((_topic: string, callback: (event: { payload: unknown }) => void) => {
      capturedCallback = callback;
      return Promise.resolve(vi.fn());
    });

    let received: FocusPayload | undefined;
    await listenFocus('s1', (payload) => {
      received = payload;
    });
    capturedCallback?.({ payload: { session_id: 's1', surface_kind: 'editor' } });

    expect(received?.session_id).toBe('s1');
    expect(received?.surface_kind).toBe('editor');
  });
});
