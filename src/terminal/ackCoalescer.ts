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
    // 既定値は queueMicrotask の裸参照にしないこと。呼び出し側は `this.schedule(fn)`
    // というメソッド呼び出し構文で呼ぶため、裸参照のままだとネイティブ queueMicrotask に
    // AckCoalescer インスタンスが `this` として渡り、ブラウザの WebIDL 実装が
    // `TypeError: Illegal invocation` を投げる（vitest の jsdom 環境ではこの this 束縛が
    // 強制されず検出できなかった。fix round 2 で e2e から実ブラウザで発見）。
    // アロー関数でラップして `this` を切り離す。
    private readonly schedule: (fn: () => void) => void = (fn) => queueMicrotask(fn),
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
    // Important 3（PR 10 fix round 1）: `scheduled` は schedule() を呼ぶ「前」に立てる。
    //
    // 以前は schedule() 呼び出しの後ろで scheduled = true していたが、これは
    // 同期スケジューラ（テスト注入の `(fn) => fn()` 等）を渡すと新しい恒久停止を生む
    // ——schedule() が同期的に flush() を実行し、flush() の先頭で scheduled を
    // false に戻した「直後」に、戻ってきた呼び出し側がここで scheduled = true を
    // 上書きしてしまい、「保留中の flush は無いのに scheduled が立ったまま」になって
    // 以後すべての consumed() が早期リターンし続ける。
    //
    // schedule() を呼ぶ前に scheduled = true しておけば、同期スケジューラでも
    // flush() が最後に立てた false が正しく残る。一方 schedule() 自体が登録に
    // 失敗して例外を投げた場合（fix round 2 で見つかった経路）は catch で
    // scheduled を false に戻し、次の consumed() で再試行できるようにする。
    this.scheduled = true;
    try {
      this.schedule(() => this.flush());
    } catch (e) {
      this.scheduled = false;
      throw e;
    }
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
