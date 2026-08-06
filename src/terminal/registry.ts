import { CanvasAddon } from '@xterm/addon-canvas';
import { FitAddon } from '@xterm/addon-fit';
import { WebglAddon } from '@xterm/addon-webgl';
import { Terminal } from '@xterm/xterm';
import type { ITerminalOptions, ITheme } from '@xterm/xterm';
import '@xterm/xterm/css/xterm.css';

import { encodeBinaryString } from '../ipc/base64';
import { ackPty, writePty, writePtyBytes } from '../ipc/commands';
import { AckCoalescer } from './ackCoalescer';
import { createTerminalKeyEventHandler } from './keyRouting';
import { loadAcceleratedRenderer, type RendererKind } from './renderer';
import './terminal.css';

/** 設計書 §8-4: エージェント用サーフェスは 10,000 行保持し DB には保存しない */
const AGENT_SCROLLBACK_LINES = 10_000;
/** 契約 §19: nvim は自前で画面を管理するので長いスクロールバックが無駄になる */
const EDITOR_SCROLLBACK_LINES = 1_000;

/** surface_id の ':' 以降が SurfaceKind（契約 §5） */
function scrollbackFor(surfaceId: string): number {
  return surfaceId.endsWith(':editor') ? EDITOR_SCROLLBACK_LINES : AGENT_SCROLLBACK_LINES;
}

interface TerminalEntry {
  term: Terminal;
  fit: FitAddon;
  host: HTMLDivElement;
  ack: AckCoalescer;
  renderer: RendererKind;
  accel: WebglAddon | CanvasAddon | null;
  opened: boolean;
  attached: boolean;
  lastSize: { cols: number; rows: number };
  /** null = 末尾追従中 */
  savedViewportY: number | null;
  /** 必達 1（契約 §16）: writeToTerminal に渡された直近の seq。PTY 再起動による後退の検知に使う */
  lastSeenSeq: number;
}

/** xterm インスタンスは Zustand の外で保持する（契約 §10 / §16） */
const entries = new Map<string, TerminalEntry>();

/**
 * xterm の theme をデザインシステムのトークンから読む（契約 §53）。
 *
 * **hex を直書きしないこと。** ターミナル面には専用のトークン系統があり、
 * `--bg-terminal` / `--text-term` / `--text-term-dim` / `--term-*` の 11 個は
 * **テーマ非依存**である（xterm 面は常にダークで、light テーマでも値が変わらない）。
 *
 * **`var(--x, #hex)` のフォールバック形も書かないこと。** フォールバックが常に効いて
 * デザインシステムが永久に反映されない（`kamux-design-system` の SKILL.md が明示的に禁じている）。
 *
 * 呼ぶのは 1 度きりでよい。上の 11 トークンはテーマ切替で変化しないため、
 * テーマ変更時に再読み込みする必要が無い（変化する系統を混ぜたら話が変わる）。
 */
function readTerminalTheme(): ITheme {
  const s = getComputedStyle(document.documentElement);
  const t = (name: string) => s.getPropertyValue(name).trim();
  return {
    background: t('--bg-terminal'),
    foreground: t('--text-term'),
    cursor: t('--text-term'),
    black: t('--term-comment'),
    red: t('--term-red'),
    green: t('--term-green'),
    yellow: t('--term-yellow'),
    blue: t('--term-blue'),
    magenta: t('--term-magenta'),
    cyan: t('--term-cyan'),
    white: t('--text-term'),
    brightBlack: t('--text-term-dim'),
    brightRed: t('--term-red'),
    brightGreen: t('--term-green'),
    brightYellow: t('--term-orange'),
    brightBlue: t('--term-blue'),
    brightMagenta: t('--term-magenta'),
    brightCyan: t('--term-cyan'),
    brightWhite: t('--text-term'),
  };
}

/**
 * xterm のフォント設定をデザインシステムのトークンから読む。
 * `components.md`「ターミナル面」: `--font-mono` / `--text-xs`〜`--text-sm`。
 *
 * tokens.css が読み込まれていない環境（テストなど）では `getComputedStyle` が
 * 空文字列 / NaN を返す。その場合はキー自体を省略して xterm の既定値に委ねる
 * （hex フォールバックと違い、design token が無いときの防御であって「値を書く」ことにはならない）。
 *
 * **`lineHeight` は意図的に渡さない（契約 §58.2）。**
 *
 * xterm の `lineHeight` は CSS の `line-height` と同じ意味の数値ではない。
 * `cell.height = Math.floor(char.height * lineHeight)` であり、`char.height` は
 * フォントサイズではなく「フォントの自然な行ボックス」（`line-height: normal` を
 * 当てて測った `offsetHeight`。実測で概ね fontSize の 1.15〜1.3 倍）である。
 * そのため `--leading-term: 1.65` をそのまま渡すと、セル高は fontSize の
 * 約 2.0〜2.15 倍になる。
 *
 * これは「実装できない」からではない。正しい倍率
 * `lineHeight = (fontSize * leadingTerm) / 自然行ボックスの高さ` は実行時に測れるので
 * literal は要らず、契約 §53 に抵触するという前回の理由づけは成り立たない。**渡さない
 * 理由は「やる価値が未確定」であること**:
 *
 * 1. `--leading-term: 1.65` が xterm のセルグリッドを意図しているか未確定。
 *    `tokens.css:87` の定義コメントは「ターミナルとコード」双方を指しており、
 *    コードブロック（DOM）向けの値が「ターミナル」の名前で共有されているだけの
 *    可能性がある。CSS 換算 1.65 は一般的な端末（概ね 1.0〜1.4）よりかなり広い
 * 2. 実測が 1 度も無い。見た目の議論を実測なしで進めない
 *
 * Task 14 の初回描画で lane-controller が実機目視して数値を出し、
 * `--leading-term` を変えるか専用トークンを足すかは team-lead が決める。
 * **安易に戻さないこと。**
 */
function readTerminalFont(): Pick<ITerminalOptions, 'fontFamily' | 'fontSize'> {
  const s = getComputedStyle(document.documentElement);
  const fontFamily = s.getPropertyValue('--font-mono').trim();
  const fontSize = parseFloat(s.getPropertyValue('--text-sm'));
  return {
    ...(fontFamily && { fontFamily }),
    ...(!Number.isNaN(fontSize) && { fontSize }),
  };
}

/** 未生成なら生成して返す。生成済みならそれを返す（冪等） */
export function ensureTerminal(surfaceId: string): Terminal {
  const existing = entries.get(surfaceId);
  if (existing) return existing.term;

  const host = document.createElement('div');
  host.className = 'kamux-term-host';
  host.style.display = 'none';

  const term = new Terminal({
    scrollback: scrollbackFor(surfaceId),
    // true にすると常時タイマーが回りアイドル CPU 0% を壊す（契約 §0）
    cursorBlink: false,
    ...readTerminalFont(),
    // 契約 §53 / デザインシステム: xterm の theme に hex を直書きしないこと。
    // ターミナル面は専用トークン系統（テーマ非依存。light でも値が変わらない）を読む。
    theme: readTerminalTheme(),
  });
  const fit = new FitAddon();
  term.loadAddon(fit);

  const ack = new AckCoalescer((seq) => {
    // 握り潰してよい理由（fix round 1・必達 1）: `RegistrySink::on_exit`（Rust）は
    // レジストリからの登録解除を先に行い、その後に `pty://exit` を emit する。
    // したがってフロントが exit を知る時点で surface は既に消えており、
    // 保留中の ack flush が `NotFound` を返すのは正常系である
    // （「もう存在しない PTY への通知」なので、失敗しても何も壊れない）。
    // `void ackPty(...)` のままだと PTY が終了するたびに unhandled promise
    // rejection が発生する（Task 13 で実測）。
    ackPty(surfaceId, seq).catch(() => {
      // 上記の理由により意図的に無視する
    });
  });

  // term.onData / onBinary の接続はここで 1 回だけ行う（契約 §16 / 1034 行）。
  // 消費側（TerminalView / EditorView / ペイン再割当）は絶対に onData / onBinary を張らないこと。
  // xterm の onData はリスナを重ねられるため、二重登録すると打鍵 1 回が 2 回 PTY に届く。
  term.onData((data) => {
    // Important 2（PR 10 fix round 1）: Task 13 必達 1 で ackPty だけに .catch を
    // 足したが、同じ理由（PTY 終了後は NotFound を返すのが正常系）が成り立つ隣の
    // write_pty / write_pty_bytes が残っていた。PTY 終了後（[process exited] 表示のまま）
    // の打鍵ごとに unhandled promise rejection が発生する。
    writePty(surfaceId, data).catch(() => {
      // 上記の理由により意図的に無視する
    });
  });
  term.onBinary((data) => {
    writePtyBytes(surfaceId, encodeBinaryString(data)).catch(() => {
      // 上記の理由により意図的に無視する
    });
  });
  // Cmd 系は window のキーマップに渡す（契約 §11）。
  // Cmd+C / Cmd+V はブラウザのネイティブ処理なので、ここで止めても効く。
  // 返り値の意味は変えない（I3）。フラグの書き戻しは副作用として行う（契約 §65.3）。
  term.attachCustomKeyEventHandler(createTerminalKeyEventHandler(term));

  entries.set(surfaceId, {
    term,
    fit,
    host,
    ack,
    renderer: 'dom',
    accel: null,
    opened: false,
    attached: false,
    lastSize: { cols: 0, rows: 0 },
    savedViewportY: null,
    lastSeenSeq: 0,
  });
  return term;
}

export function getTerminal(surfaceId: string): Terminal | undefined {
  return entries.get(surfaceId)?.term;
}

export function listTerminals(): string[] {
  return Array.from(entries.keys());
}

function saveScroll(entry: TerminalEntry): void {
  const buffer = entry.term.buffer.active;
  entry.savedViewportY = buffer.viewportY >= buffer.baseY ? null : buffer.viewportY;
}

function restoreScroll(entry: TerminalEntry): void {
  if (entry.savedViewportY === null) {
    entry.term.scrollToBottom();
    return;
  }
  entry.term.scrollToLine(entry.savedViewportY);
}

function downgradeToDom(entry: TerminalEntry): void {
  if (entry.accel === null) return;
  entry.accel.dispose();
  entry.accel = null;
  entry.renderer = 'dom';
}

/**
 * DOM コンテナへ接続する。別のコンテナに接続済みなら、
 * スクロールバックを保ったまま付け替える（契約 §16）。
 * 初回はここで term.open() する（DOM に入ってから font を測定させるため）。
 */
export function attachTerminal(surfaceId: string, container: HTMLElement): void {
  ensureTerminal(surfaceId);
  const entry = entries.get(surfaceId);
  if (!entry) return;

  if (entry.host.parentElement !== container) {
    container.appendChild(entry.host);
  }
  entry.host.style.display = 'block';
  entry.attached = true;

  if (!entry.opened) {
    entry.term.open(entry.host);
    entry.opened = true;
  }
  if (entry.accel === null) {
    const loaded = loadAcceleratedRenderer(entry.term, {
      webgl: () => new WebglAddon(),
      canvas: () => new CanvasAddon(),
    });
    entry.renderer = loaded.renderer;
    entry.accel = loaded.addon;
    if (loaded.addon instanceof WebglAddon) {
      loaded.addon.onContextLoss(() => downgradeToDom(entry));
    }
  }

  restoreScroll(entry);
  // term.focus() はここでは呼ばない（PR 10 fix round 1 の Critical）。
  // ここは契約 §16 に無い副作用で、モーダル表示中の Cmd+2 再マウントでも
  // 無条件に発火すると、実シェルの PTY が DOM フォーカスを奪ってしまい、
  // モーダルへの入力が打鍵ごとに write_pty として実シェルへ流れる
  // （§11.4.3 が許容したのは「見た目の違和感」であって「入力が別の宛先で
  // 実行される」ことではない）。呼び出し側（TerminalPane）が
  // modal === null を確認したうえで getTerminal(surfaceId)?.focus() を呼ぶ。
}

/** DOM から切り離すがインスタンスとスクロールバックは保持する（契約 §16） */
export function detachTerminal(surfaceId: string): void {
  const entry = entries.get(surfaceId);
  if (!entry) return;
  saveScroll(entry);
  // 見えていないサーフェスは WebGL コンテキストを手放す（同時コンテキスト数の上限対策）
  downgradeToDom(entry);
  entry.host.style.display = 'none';
  entry.attached = false;
}

/** インスタンスを破棄する。PTY 終了かセッション削除時のみ呼ぶ（契約 §16） */
export function disposeTerminal(surfaceId: string): void {
  const entry = entries.get(surfaceId);
  if (!entry) return;
  entry.accel?.dispose();
  entry.term.dispose();
  entry.host.remove();
  entries.delete(surfaceId);
}

/**
 * FitAddon で寸法を再計算する。変化がなければ null（契約 §16）。
 * resize_pty を呼ぶのは呼び出し側の責務。
 */
export function fitTerminal(surfaceId: string): { cols: number; rows: number } | null {
  const entry = entries.get(surfaceId);
  if (!entry || !entry.attached) return null;
  const rect = entry.host.getBoundingClientRect();
  // display:none の間は 0x0 になる。0 列 0 行を PTY に送ると子プロセスが壊れる
  if (rect.width < 1 || rect.height < 1) return null;
  entry.fit.fit();
  const { cols, rows } = entry.term;
  if (cols === entry.lastSize.cols && rows === entry.lastSize.rows) return null;
  entry.lastSize = { cols, rows };
  return { cols, rows };
}

/**
 * `fitTerminal` の直近サイズキャッシュを無効化する（M1-3 の追加。契約 §16 の 8 関数ではない）。
 *
 * fix round 1 で発見: PTY 再起動時、`fitTerminal` は寸法が前回と同じであれば `null` を返し
 * `resize_pty` を飛ばす。一方 `start_session` は固定 80×24 で spawn するため、
 * 再起動された PTY だけ 80 桁のまま xterm 側の実寸に追従しない
 * （初回起動は `lastSize` が `{0,0}` から始まるので影響を受けない）。
 *
 * `detachTerminal` で `lastSize` を戻すだけでは直らない
 * （`syncSize` が `start_session` より前に走る順序は変わらないため）。
 * 呼び出し側（Task 14）が `startSession` の解決後にこれを呼んでから `fitTerminal` /
 * `resizePty` をもう一度実行する。
 */
export function invalidateFitCache(surfaceId: string): void {
  const entry = entries.get(surfaceId);
  if (!entry) return;
  entry.lastSize = { cols: 0, rows: 0 };
}

/**
 * PTY 出力の書き込み。消化完了後に ack_pty を送るのはこの関数の責務（契約 §16）。
 * xterm はステートフルな UTF-8 デコーダを持つので、
 * チャンク境界がマルチバイト文字を割っても正しく繋がる。
 *
 * 必達 1（M1-3 Task 10 レビューで発見）: 同じ surface_id で PTY が再起動すると
 * Rust 側の seq は 1 から振り直される。契約 §16 の規則で disposeTerminal は
 * PTY 終了時にしか呼ばないため、registry のエントリと AckCoalescer は
 * 前世代の lastSent を持ったまま残る。
 *
 * この関数（投入側）は単一 reader スレッドが seq 昇順で emit するため厳密に単調増加であり、
 * 「順不同」と「世代交代」を弁別できる唯一の場所である。ここでの後退検知が主機構。
 * AckCoalescer.consumed 側の自己検知（発火順序が xterm の書き込みキュー依存で
 * 順不同と世代交代を原理的に弁別できない）はあくまで保険であり、削除しないこと。
 */
export function writeToTerminal(surfaceId: string, bytes: Uint8Array, seq: number): void {
  ensureTerminal(surfaceId);
  const entry = entries.get(surfaceId);
  if (!entry) return;
  if (seq < entry.lastSeenSeq) {
    entry.ack.reset();
  }
  entry.lastSeenSeq = seq;
  entry.term.write(bytes, () => entry.ack.consumed(seq));
}

/** 起動失敗などをターミナル自身に出す（トースト基盤に依存しない） */
export function writeNotice(
  surfaceId: string,
  message: string,
  tone: 'info' | 'error' = 'info',
): void {
  ensureTerminal(surfaceId);
  const entry = entries.get(surfaceId);
  if (!entry) return;
  const color = tone === 'error' ? '\x1b[31m' : '\x1b[2m';
  entry.term.write(`\r\n${color}${message}\x1b[0m\r\n`);
}
