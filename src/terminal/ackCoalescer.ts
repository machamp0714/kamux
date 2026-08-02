/**
 * xterm の write コールバックで確定した seq を 1 フラッシュ 1 回にまとめて ack する。
 * タイマーを使わず queueMicrotask で集約するので、出力が無いときは一切動かない（契約 §0）。
 */
export class AckCoalescer {
  private highestConsumed = 0;

  private lastSent = 0;

  private scheduled = false;

  constructor(
    private readonly send: (seq: number) => void,
    private readonly schedule: (fn: () => void) => void = queueMicrotask,
  ) {}

  /** xterm がそのチャンクを消化したときに呼ぶ */
  consumed(seq: number): void {
    // seq の後退 = 同じ surface_id で PTY が再起動された（Rust 側 seq は 1 から）。
    // 契約 §16 は disposeTerminal を「PTY 終了かセッション削除時のみ」に限っているため、
    // 再起動では registry のエントリが残り続ける。ここで自己修復する。
    //
    // 判定基準は「まだ未送信の highestConsumed」ではなく「既に Rust へ ack 済みの lastSent」。
    // 同一フラッシュ内で順不同に届いた seq（例: 5 → 2）を世代交代と誤検知しないため。
    if (seq < this.lastSent) {
      this.reset();
    }
    if (seq > this.highestConsumed) {
      this.highestConsumed = seq;
    }
    if (this.scheduled) return;
    this.scheduled = true;
    this.schedule(() => this.flush());
  }

  /** PTY が終了して同じ surface_id で再起動されると Rust 側の seq は 1 に戻る */
  reset(): void {
    this.highestConsumed = 0;
    this.lastSent = 0;
  }

  private flush(): void {
    this.scheduled = false;
    if (this.highestConsumed <= this.lastSent) return;
    this.lastSent = this.highestConsumed;
    this.send(this.lastSent);
  }
}
