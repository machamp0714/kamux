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
    // `scheduled` は schedule() が実際に登録できてから立てる（呼ぶ前ではない）。
    // queueMicrotask は同期的にコールバックを実行しないため、この並び替えは
    // 成功時の挙動を一切変えない。変えるのは schedule() 自体が例外を投げた場合だけで、
    // その場合 `scheduled` は false のまま残り、次の consumed() で再試行できる。
    // fix round 2 で見つかったバグは「scheduled を先に立てていたため、schedule() が
    // 例外を投げると恒久的に ack が止まる」という壊れ方だった。ここでの並び替えは
    // その種の再発（schedule 実装側の別の失敗）を「1 回分の ack 遅延」に格下げする。
    this.schedule(() => this.flush());
    this.scheduled = true;
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
