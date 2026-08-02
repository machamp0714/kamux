import { describe, expect, it, vi } from 'vitest';
import { AckCoalescer } from './ackCoalescer';

/** スケジューラを手動で回すためのヘルパ */
function manualScheduler() {
  const queue: Array<() => void> = [];
  return {
    schedule: (fn: () => void) => queue.push(fn),
    flush: () => {
      while (queue.length > 0) {
        const fn = queue.shift();
        if (fn) fn();
      }
    },
  };
}

describe('AckCoalescer', () => {
  it('同一フラッシュ内の複数チャンクを最大 seq 1 回にまとめる', () => {
    const send = vi.fn();
    const scheduler = manualScheduler();
    const ack = new AckCoalescer(send, scheduler.schedule);

    ack.consumed(1);
    ack.consumed(2);
    ack.consumed(3);
    expect(send).not.toHaveBeenCalled();

    scheduler.flush();
    expect(send).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenCalledWith(3);
  });

  it('順不同で届いても最大値を送る', () => {
    const send = vi.fn();
    const scheduler = manualScheduler();
    const ack = new AckCoalescer(send, scheduler.schedule);

    ack.consumed(5);
    ack.consumed(2);
    scheduler.flush();
    expect(send).toHaveBeenCalledWith(5);
  });

  it('既に送った seq 以下では再送しない', () => {
    const send = vi.fn();
    const scheduler = manualScheduler();
    const ack = new AckCoalescer(send, scheduler.schedule);

    ack.consumed(4);
    scheduler.flush();
    ack.consumed(4);
    scheduler.flush();
    expect(send).toHaveBeenCalledTimes(1);
  });

  it('進捗があれば次のフラッシュで送る', () => {
    const send = vi.fn();
    const scheduler = manualScheduler();
    const ack = new AckCoalescer(send, scheduler.schedule);

    ack.consumed(4);
    scheduler.flush();
    ack.consumed(9);
    scheduler.flush();
    expect(send.mock.calls).toEqual([[4], [9]]);
  });

  it('reset 後は seq が 1 に戻っても ack する（PTY 再起動で Rust 側 seq がリセットされる）', () => {
    const send = vi.fn();
    const scheduler = manualScheduler();
    const ack = new AckCoalescer(send, scheduler.schedule);

    ack.consumed(5000);
    scheduler.flush();
    expect(send).toHaveBeenLastCalledWith(5000);

    ack.reset();
    ack.consumed(1);
    scheduler.flush();
    expect(send).toHaveBeenLastCalledWith(1);
    expect(send).toHaveBeenCalledTimes(2);
  });

  it('seq が後退したら自動で reset する（PTY 再起動を明示通知されなくても回復する）', () => {
    const send = vi.fn();
    const scheduler = manualScheduler();
    const ack = new AckCoalescer(send, scheduler.schedule);

    ack.consumed(5000);
    scheduler.flush();

    // 同じ surface_id で再 spawn されると Rust 側の seq は 1 から振り直される
    ack.consumed(1);
    scheduler.flush();
    expect(send).toHaveBeenLastCalledWith(1);

    ack.consumed(2);
    scheduler.flush();
    expect(send).toHaveBeenLastCalledWith(2);
  });

  it('スケジュールは 1 回のフラッシュにつき 1 度だけ積まれる', () => {
    const send = vi.fn();
    const schedule = vi.fn();
    const ack = new AckCoalescer(send, schedule);

    ack.consumed(1);
    ack.consumed(2);
    ack.consumed(3);
    expect(schedule).toHaveBeenCalledTimes(1);
  });
});
