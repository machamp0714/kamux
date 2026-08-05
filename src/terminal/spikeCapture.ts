import type { Terminal } from '@xterm/xterm';

// SPIKE(§65.5): Task 4 で除去する
//
// これは「!」を直す手当てではない（手当ては Task 3。契約 §65.3）。実機（WKWebView + IME）
// で 6 経路のイベント順序を測るための使い捨て計装であり、DevTools を開かせずに
// 画面オーバーレイへログを溜める。読み取り経路は今後の手当てが使う経路と逐語で一致させて
// あること（`term._core._keyDownSeen`。契約 §65 / xtermCanary.test.ts の T4 と同じ経路）。

/**
 * `@xterm/xterm` の private フィールド。配布物でプロパティ名は難読化されていない
 * （契約 §65.2 の逐語がその証拠）。`any` を避けるための局所的な型宣言。
 */
interface XtermCoreProbe {
  _keyDownSeen?: unknown;
  _keyPressHandled?: unknown;
}

interface TerminalWithCore {
  _core?: XtermCoreProbe;
}

/** 3.3 の 12 フィールド。1 行 = 1 イベント（onData 経由の行も同じ形で混ぜる） */
interface SpikeLogRow {
  step: number;
  /** 最初のイベントからの相対 ms。小数第 1 位 */
  t: number;
  type: string;
  key: string | null;
  code: string | null;
  keyCode: number | null;
  isComposing: boolean | null;
  inputType: string | null;
  data: string | null;
  composed: boolean | null;
  _keyDownSeen: boolean | null;
  _keyPressHandled: boolean | null;
}

/** 3.2: この 8 種すべてを document に capture:true で張る */
const CAPTURED_EVENT_TYPES = [
  'keydown',
  'keypress',
  'keyup',
  'beforeinput',
  'input',
  'compositionstart',
  'compositionupdate',
  'compositionend',
] as const;

/** 3.5 の 12 ステップ。この文言をそのまま埋め込む */
const STEP_LABELS: readonly string[] = [
  ' 1. 【日本語】! を 2 連打',
  ' 2. 【日本語】@',
  ' 3. 【日本語】Shift+英字を 3 文字',
  ' 4. 【日本語】素の英字を 3 文字',
  ' 5. 【日本語】日本語を変換確定',
  ' 6. 【日本語】Cmd+V で貼り付け',
  ' 7. 【ABC】! を 2 連打',
  ' 8. 【ABC】@',
  ' 9. 【ABC】Shift+英字を 3 文字',
  '10. 【ABC】素の英字を 3 文字',
  '11. 【ABC】ローマ字をそのまま打って Enter（ABC では変換は起きない。飛ばしてもよい）',
  '12. 【ABC】Cmd+V で貼り付け',
];

let startTime: number | null = null;
let currentStep = 0;
let logTextareaEl: HTMLTextAreaElement | null = null;
let stepLabelEl: HTMLSpanElement | null = null;
let controlResultEl: HTMLDivElement | null = null;

function relativeMs(): number {
  const now = performance.now();
  if (startTime === null) startTime = now;
  return Math.round((now - startTime) * 10) / 10;
}

function appendLine(line: string): void {
  // 3.4: クリップボードが WKWebView で失敗したときの保険として、各行を console.log にも出す
  console.log(line);
  if (!logTextareaEl) return;
  logTextareaEl.value += `${line}\n`;
  logTextareaEl.scrollTop = logTextareaEl.scrollHeight;
}

function appendRow(row: SpikeLogRow): void {
  appendLine(JSON.stringify(row));
}

/** 3.6: ステップ 1 の行を走査し、コントロール判定を行う */
function runControlCheck(): void {
  if (!logTextareaEl || !controlResultEl) return;
  const step1Lines = logTextareaEl.value
    .split('\n')
    .filter((line) => line.trim().startsWith('{'))
    .map((line) => JSON.parse(line) as SpikeLogRow)
    .filter((row) => row.step === 1);

  const inputRow = step1Lines.find((row) => row.type === 'input');
  const ok = inputRow !== undefined && inputRow._keyDownSeen === true;

  if (ok) {
    controlResultEl.textContent = 'コントロール OK: 計装は信号を取れている';
    controlResultEl.style.color = '#4caf50';
  } else {
    const reason =
      inputRow === undefined
        ? 'input イベントの行が無い'
        : `_keyDownSeen が ${String(inputRow._keyDownSeen)}`;
    controlResultEl.textContent = `コントロール NG: ${reason}。ここで中止して報告してください`;
    controlResultEl.style.color = '#f44336';
  }
}

function goToStep(n: number): void {
  const wasStep1 = currentStep === 1;
  currentStep = n;
  if (stepLabelEl) {
    stepLabelEl.textContent = STEP_LABELS[n - 1] ?? '';
  }
  appendLine(`--- STEP ${n}: ${STEP_LABELS[n - 1] ?? ''} ---`);
  if (wasStep1 && n === 2) {
    runControlCheck();
  }
}

/**
 * オーバーレイの全要素をタブ移動から外す（3.4 の ⚠️: ターミナルからフォーカスを奪わないこと）。
 * `mousedown` の `preventDefault()` は**ボタンだけ**に限る —— brief 3.4 の逐語どおり。
 * ログ textarea にまで掛けると、3.4 のクリップボード失敗時フォールバック（`select()`）が
 * 自分自身で封じられてしまう（textarea をクリックしても選択状態に入れなくなる）。
 */
function excludeFromTabOrder(el: HTMLElement): void {
  el.tabIndex = -1;
}

/** ボタンでフォーカスを奪わないための共通セットアップ（3.4 の ⚠️） */
function preventFocusStealButton(el: HTMLButtonElement): void {
  excludeFromTabOrder(el);
  el.addEventListener('mousedown', (e) => {
    e.preventDefault();
  });
}

function copyLogToClipboard(): void {
  if (!logTextareaEl) return;
  const text = logTextareaEl.value;
  navigator.clipboard.writeText(text).catch(() => {
    // フォールバック: textarea を select() してユーザーに手動コピーさせる
    logTextareaEl?.select();
  });
}

/** 3.4: DevTools を開かせない画面オーバーレイ。React には触らず document.body に直接生成する */
function createOverlay(): void {
  // 新しいオーバーレイ = 新しい計測セッション。タイムラインの原点とステップをリセットする
  startTime = null;
  currentStep = 0;

  const overlay = document.createElement('div');
  overlay.id = 'kamux-spike-overlay';
  excludeFromTabOrder(overlay);
  overlay.style.position = 'fixed';
  overlay.style.right = '0';
  overlay.style.bottom = '0';
  overlay.style.width = '45vw';
  overlay.style.height = '45vh';
  overlay.style.zIndex = '2147483647';
  overlay.style.background = '#111';
  overlay.style.color = '#eee';
  overlay.style.fontFamily = 'monospace';
  overlay.style.fontSize = '11px';
  overlay.style.display = 'flex';
  overlay.style.flexDirection = 'column';
  overlay.style.padding = '4px';
  overlay.style.boxSizing = 'border-box';

  const header = document.createElement('div');
  header.style.display = 'flex';
  header.style.justifyContent = 'space-between';
  header.style.alignItems = 'center';

  const stepLabel = document.createElement('span');
  stepLabel.id = 'kamux-spike-step-label';
  stepLabel.textContent = STEP_LABELS[0] ?? '';
  excludeFromTabOrder(stepLabel);
  stepLabelEl = stepLabel;

  const nextButton = document.createElement('button');
  nextButton.id = 'kamux-spike-next';
  nextButton.textContent = '次のステップへ';
  preventFocusStealButton(nextButton);
  nextButton.addEventListener('click', () => {
    const next = currentStep >= STEP_LABELS.length ? STEP_LABELS.length : currentStep + 1;
    goToStep(next);
  });

  header.appendChild(stepLabel);
  header.appendChild(nextButton);

  const controlResult = document.createElement('div');
  controlResult.id = 'kamux-spike-control';
  excludeFromTabOrder(controlResult);
  controlResultEl = controlResult;

  const logTextarea = document.createElement('textarea');
  logTextarea.id = 'kamux-spike-log';
  logTextarea.readOnly = true;
  // ⚠️ ここは preventFocusStealButton ではなく excludeFromTabOrder（タブ移動から外すだけ）。
  // mousedown の preventDefault まで掛けると、3.4 のクリップボード失敗時フォールバック
  // （`select()`）がここで自分自身を封じてしまう。
  excludeFromTabOrder(logTextarea);
  logTextarea.style.flex = '1';
  logTextarea.style.width = '100%';
  logTextarea.style.marginTop = '4px';
  logTextarea.style.background = '#000';
  logTextarea.style.color = '#0f0';
  logTextareaEl = logTextarea;

  const copyButton = document.createElement('button');
  copyButton.id = 'kamux-spike-copy';
  copyButton.textContent = 'コピー';
  preventFocusStealButton(copyButton);
  copyButton.addEventListener('click', copyLogToClipboard);

  overlay.appendChild(header);
  overlay.appendChild(controlResult);
  overlay.appendChild(logTextarea);
  overlay.appendChild(copyButton);

  document.body.appendChild(overlay);
  goToStep(1);
}

/** term._core からフラグを読む。読めなければ null（undefined を握りつぶさない） */
function readCoreFlags(term: Terminal): {
  keyDownSeen: boolean | null;
  keyPressHandled: boolean | null;
} {
  const core = (term as unknown as TerminalWithCore)._core;
  const keyDownSeen = typeof core?._keyDownSeen === 'boolean' ? core._keyDownSeen : null;
  const keyPressHandled =
    typeof core?._keyPressHandled === 'boolean' ? core._keyPressHandled : null;
  return { keyDownSeen, keyPressHandled };
}

/**
 * SPIKE(§65.5): Task 4 で除去する。
 *
 * document に capture:true でリスナを張る（xterm 自身のリスナは textarea 上にあり、
 * capture 段の document リスナはそれより前に走る。これがフラグの
 * 「xterm が見る直前の値」を取る唯一の方法である。契約 §65.5 / brief 3.2）。
 *
 * `ensureTerminal` は `attachTerminal` 以外（`writeToTerminal` / `writeNotice`）からも
 * 呼ばれるため、`open()` されていない（＝ `textarea` が無い）Terminal が同時に
 * 複数存在しうる。`installSpikeCapture` を term ごとに呼ぶたびに document へ
 * 8 本ずつリスナを重ねると、1 回の打鍵が複数 term ぶん重複記録され、
 * 3.6 のコントロール判定が最初に登録された（無関係な）term の行を拾って
 * 誤って NG を出しうる。**リスナは document に一度だけ張り、どの term の
 * イベントかをイベントごとに 1 つ選ぶ。**
 */
const installedTerms: Terminal[] = [];
let listenersAttached = false;

/**
 * イベントに対応する term を 1 つだけ選ぶ。
 * 1. `textarea` を持つ term のうち、`event.target` と一致するもの（実際にフォーカスされている term）
 * 2. 無ければ `textarea` がまだ無い term のうち、最後に installSpikeCapture された（＝最新の）もの
 *    —— open() 前の Terminal でも headless 検証（成果物 D）が行を作れるようにするための、
 *    意図的な規則（brief 3.2）。実物の型は `HTMLTextAreaElement | undefined`
 * 3. どちらも無ければ対象外（`undefined`）
 */
function pickTerm(event: Event): Terminal | undefined {
  const matched = installedTerms.find((t) => t.textarea && t.textarea === event.target);
  if (matched) return matched;
  for (let i = installedTerms.length - 1; i >= 0; i -= 1) {
    if (!installedTerms[i].textarea) return installedTerms[i];
  }
  return undefined;
}

function handleCapturedEvent(event: Event): void {
  const term = pickTerm(event);
  if (!term) return;

  const { keyDownSeen, keyPressHandled } = readCoreFlags(term);
  const kbd = event as Partial<KeyboardEvent>;
  const inputEv = event as Partial<InputEvent>;

  const row: SpikeLogRow = {
    step: currentStep,
    t: relativeMs(),
    type: event.type,
    key: typeof kbd.key === 'string' ? kbd.key : null,
    code: typeof kbd.code === 'string' ? kbd.code : null,
    keyCode: typeof kbd.keyCode === 'number' ? kbd.keyCode : null,
    isComposing: typeof kbd.isComposing === 'boolean' ? kbd.isComposing : null,
    inputType: typeof inputEv.inputType === 'string' ? inputEv.inputType : null,
    data: typeof inputEv.data === 'string' ? inputEv.data : null,
    composed: typeof event.composed === 'boolean' ? event.composed : null,
    _keyDownSeen: keyDownSeen,
    _keyPressHandled: keyPressHandled,
  };
  appendRow(row);
}

export function installSpikeCapture(term: Terminal): void {
  if (!installedTerms.includes(term)) {
    installedTerms.push(term);
  }

  if (!listenersAttached) {
    listenersAttached = true;
    for (const type of CAPTURED_EVENT_TYPES) {
      document.addEventListener(type, handleCapturedEvent, { capture: true });
    }
  }

  ensureOverlay();
}

/**
 * オーバーレイの存在を DOM 上で判定する（モジュール変数の真偽フラグではなく）。
 * headless テスト（成果物 D）はテストごとに `#kamux-spike-overlay` を DOM から
 * 取り除いて次のテストへ進む。真偽フラグ方式だと 2 回目以降 `createOverlay` が
 * 呼ばれず、参照が外れた textarea へ書き続けて記録が見えなくなる。
 */
function ensureOverlay(): void {
  if (document.getElementById('kamux-spike-overlay')) return;
  createOverlay();
}

/**
 * SPIKE(§65.5): Task 4 で除去する。
 *
 * xterm が実際に PTY へ流した文字（onData）を、キー/IME イベントと同じタイムラインに
 * 1 行として混ぜる。どの経路が文字を入れたかを決める決定的な信号（brief 3.3）。
 */
export function logSpikeData(surfaceId: string, data: string): void {
  const row: SpikeLogRow = {
    step: currentStep,
    t: relativeMs(),
    type: 'onData',
    // 3.3 のフィールドに surfaceId は無いが、複数サーフェスが同時に spike へ乗る
    // 可能性を捨てないため code 欄に間借りする（key/code/keyCode は onData には無縁）
    key: null,
    code: surfaceId,
    keyCode: null,
    isComposing: null,
    inputType: null,
    data,
    composed: null,
    _keyDownSeen: null,
    _keyPressHandled: null,
  };
  appendRow(row);
}
