import { describe, expect, it, vi } from 'vitest';
import { loadAcceleratedRenderer } from './renderer';

/** loadAddon だけ持つ最小の Terminal 代替 */
function fakeTerm() {
  return { loadAddon: vi.fn() } as unknown as Parameters<typeof loadAcceleratedRenderer>[0];
}

describe('loadAcceleratedRenderer', () => {
  it('WebGL が使えるときは webgl を選ぶ', () => {
    const term = fakeTerm();
    const webglAddon = { name: 'webgl' };
    const result = loadAcceleratedRenderer(term, {
      webgl: () => webglAddon as never,
      canvas: () => ({ name: 'canvas' }) as never,
    });
    expect(result.renderer).toBe('webgl');
    expect(result.addon).toBe(webglAddon);
    expect(term.loadAddon).toHaveBeenCalledWith(webglAddon);
  });

  it('WebGL の生成が throw したら canvas に落ちる', () => {
    const term = fakeTerm();
    const canvasAddon = { name: 'canvas' };
    const result = loadAcceleratedRenderer(term, {
      webgl: () => {
        throw new Error('WebGL2 unavailable');
      },
      canvas: () => canvasAddon as never,
    });
    expect(result.renderer).toBe('canvas');
    expect(result.addon).toBe(canvasAddon);
  });

  it('loadAddon が throw しても canvas に落ちる', () => {
    const loadAddon = vi.fn().mockImplementationOnce(() => {
      throw new Error('context creation failed');
    });
    const term = { loadAddon } as unknown as Parameters<typeof loadAcceleratedRenderer>[0];
    const result = loadAcceleratedRenderer(term, {
      webgl: () => ({ name: 'webgl' }) as never,
      canvas: () => ({ name: 'canvas' }) as never,
    });
    expect(result.renderer).toBe('canvas');
  });

  it('両方失敗したら DOM レンダラ（アドオン無し）になる', () => {
    const term = fakeTerm();
    const result = loadAcceleratedRenderer(term, {
      webgl: () => {
        throw new Error('no webgl');
      },
      canvas: () => {
        throw new Error('no canvas');
      },
    });
    expect(result.renderer).toBe('dom');
    expect(result.addon).toBeNull();
  });
});
