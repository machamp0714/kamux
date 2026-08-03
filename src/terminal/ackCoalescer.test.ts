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

  it('フラッシュ保留中に reset されたら死んだ世代の ack を撃たない', () => {
    const send = vi.fn();
    const scheduler = manualScheduler();
    const ack = new AckCoalescer(send, scheduler.schedule);

    ack.consumed(5000); // フラッシュ未実行のまま
    // Minor 1（PR 10 fix round 1）: 「Task 13 が pty://exit で呼ぶ経路」は事実と違う
    // ——ptyBridge の exit ハンドラは reset() を呼ばない。実際の自動回復の主機構は
    // registry.ts の writeToTerminal が seq の後退を検知して呼ぶ経路である
    // （PTY 再起動で Rust 側の seq が 1 から振り直されたときの自己修復）。
    ack.reset();
    scheduler.flush();
    expect(send).not.toHaveBeenCalled();

    ack.consumed(1); // 新世代
    scheduler.flush();
    expect(send).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenCalledWith(1);
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

  it('既定スケジューラ（queueMicrotask）は同期実行せずマイクロタスク経由で送る', async () => {
    const send = vi.fn();
    const ack = new AckCoalescer(send); // schedule を省略 = 本番の既定引数

    ack.consumed(1);
    expect(send).not.toHaveBeenCalled();

    await Promise.resolve();
    expect(send).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenCalledWith(1);
  });

  it('Important 3（PR 10 fix round 1）: 同期スケジューラでも 2 回目以降の consumed が恒久的に止まらない', () => {
    // fix round 1 の並び替え（scheduled = true を schedule() 呼び出しの「後」に置く形）は
    // 非同期スケジューラでは正しいが、同期スケジューラ（呼んだ瞬間に flush() が走る）を
    // 注入すると新しい恒久停止を生んでいた: flush() が scheduled を false に戻した直後に、
    // 戻ってきた schedule() 呼び出し側が scheduled = true で再び上書きしてしまい、
    // 「保留中の flush は無いのに scheduled が立ったまま」になって以後の consumed が
    // 全部 if (this.scheduled) return; で早期リターンしていた。
    const send = vi.fn();
    const ack = new AckCoalescer(send, (fn) => fn()); // 同期スケジューラ

    ack.consumed(1);
    ack.consumed(2);
    ack.consumed(3);

    expect(send.mock.calls).toEqual([[1], [2], [3]]);
  });

  it('schedule() が登録に失敗して例外を投げても、次の consumed() で再試行できる（fix round 2 の経路を try/catch で維持）', () => {
    const send = vi.fn();
    let shouldThrow = true;
    const schedule = vi.fn((fn: () => void) => {
      if (shouldThrow) throw new Error('registration failed');
      fn();
    });
    const ack = new AckCoalescer(send, schedule);

    expect(() => ack.consumed(1)).toThrow('registration failed');
    expect(send).not.toHaveBeenCalled();

    shouldThrow = false;
    ack.consumed(2);
    expect(send).toHaveBeenCalledWith(2);
  });

  it('Important 4（PR 10 fix round 1）: 既定スケジューラは this を AckCoalescer に束縛せずに queueMicrotask を呼ぶ', async () => {
    // 実ブラウザでの元バグ: schedule の既定引数が queueMicrotask の裸参照だと、
    // consumed() 内の `this.schedule(fn)`（メソッド呼び出し構文）が queueMicrotask を
    // AckCoalescer インスタンスを this として呼び出し、ネイティブ実装が
    // TypeError: Illegal invocation を投げていた（vitest の jsdom は this 束縛を
    // 強制しないため、illegal invocation そのものは再現できない。ここでは
    // 「呼び出し時の this が何であったか」を直接検査することで同じ回帰を検出する）。
    const originalQueueMicrotask = globalThis.queueMicrotask;
    let wasCalled = false;
    let thisWasAckInstance = false;
    let thisWasUndefined = false;
    // `this` をそのまま変数へエイリアスすると @typescript-eslint/no-this-alias に
    // 抵触するため、判定結果（真偽値）だけを外側へ持ち出す
    globalThis.queueMicrotask = function (fn: () => void): void {
      wasCalled = true;
      thisWasAckInstance = this instanceof AckCoalescer;
      thisWasUndefined = this === undefined;
      originalQueueMicrotask(fn);
    };

    try {
      const send = vi.fn();
      const ack = new AckCoalescer(send); // schedule を省略 = 本番の既定引数

      ack.consumed(1);

      expect(wasCalled).toBe(true);
      // 裸参照（schedule = queueMicrotask）に戻す変異では、this.schedule(fn) が
      // メソッド呼び出し構文になるため this は ack インスタンスになる。
      // 現状（アロー関数でラップ）では queueMicrotask(fn) は素の関数呼び出しなので
      // strict mode により this は undefined になる
      expect(thisWasAckInstance).toBe(false);
      expect(thisWasUndefined).toBe(true);

      await Promise.resolve();
      expect(send).toHaveBeenCalledWith(1);
    } finally {
      globalThis.queueMicrotask = originalQueueMicrotask;
    }
  });
});
