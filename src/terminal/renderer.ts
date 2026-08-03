import type { CanvasAddon } from '@xterm/addon-canvas';
import type { WebglAddon } from '@xterm/addon-webgl';
import type { Terminal } from '@xterm/xterm';

export type RendererKind = 'webgl' | 'canvas' | 'dom';

/**
 * WebGL -> Canvas -> DOM の順でレンダラを試す。
 * 可視サーフェスにだけ載せるので、同時 WebGL コンテキスト数の上限に触れない。
 */
export function loadAcceleratedRenderer(
  term: Pick<Terminal, 'loadAddon'>,
  factories: { webgl: () => WebglAddon; canvas: () => CanvasAddon },
): { renderer: RendererKind; addon: WebglAddon | CanvasAddon | null } {
  try {
    const addon = factories.webgl();
    term.loadAddon(addon);
    return { renderer: 'webgl', addon };
  } catch {
    // WebGL2 非対応・GPU プロセス不在・コンテキスト数上限
  }
  try {
    const addon = factories.canvas();
    term.loadAddon(addon);
    return { renderer: 'canvas', addon };
  } catch {
    // 何も載せなければ xterm 既定の DOM レンダラで動く
  }
  return { renderer: 'dom', addon: null };
}
