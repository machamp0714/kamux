import { describe, it, expect, vi } from 'vitest';
import { createFitScheduler } from './fitScheduler';

/** rAF を手動で進めるフェイク。 */
function fakeRaf() {
  const queue = new Map<number, () => void>();
  let id = 0;
  return {
    raf: (cb: () => void) => {
      id += 1;
      queue.set(id, cb);
      return id;
    },
    caf: (h: number) => {
      queue.delete(h);
    },
    tick: () => {
      const cbs = [...queue.values()];
      queue.clear();
      cbs.forEach((cb) => cb());
    },
    pending: () => queue.size,
  };
}

describe('createFitScheduler', () => {
  it('同一フレーム内の複数要求を 1 回に畳む', () => {
    const flush = vi.fn();
    const f = fakeRaf();
    const s = createFitScheduler(flush, f.raf, f.caf);

    s.request();
    s.request();
    s.request();
    expect(flush).not.toHaveBeenCalled();

    f.tick();
    expect(flush).toHaveBeenCalledTimes(1);
  });

  it('flush 後の新しい要求は再びスケジュールされる', () => {
    const flush = vi.fn();
    const f = fakeRaf();
    const s = createFitScheduler(flush, f.raf, f.caf);

    s.request();
    f.tick();
    s.request();
    f.tick();
    expect(flush).toHaveBeenCalledTimes(2);
  });

  it('要求しなければ 1 度も rAF を積まない（アイドル CPU ほぼ 0%）', () => {
    const flush = vi.fn();
    const f = fakeRaf();
    createFitScheduler(flush, f.raf, f.caf);
    expect(f.pending()).toBe(0);
  });

  it('cancel で保留中の要求を取り消す', () => {
    const flush = vi.fn();
    const f = fakeRaf();
    const s = createFitScheduler(flush, f.raf, f.caf);

    s.request();
    s.cancel();
    f.tick();
    expect(flush).not.toHaveBeenCalled();
  });

  it('cancel 後も再度 request できる', () => {
    const flush = vi.fn();
    const f = fakeRaf();
    const s = createFitScheduler(flush, f.raf, f.caf);

    s.request();
    s.cancel();
    s.request();
    f.tick();
    expect(flush).toHaveBeenCalledTimes(1);
  });
});
