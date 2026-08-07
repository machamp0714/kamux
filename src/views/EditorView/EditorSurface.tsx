import { useEffect, useRef, useState } from 'react';

import { resizePty, spawnEditor } from '../../ipc/commands';
import { onPtyExit } from '../../ipc/events';
import { useAppStore } from '../../store';
import { toAppError } from '../../store/uiSlice';
import { applyEditorTerminalOptions } from '../../terminal/editorTerminalOptions';
import { ensurePtySubscription } from '../../terminal/ptyBridge';
import {
  attachTerminal,
  detachTerminal,
  ensureTerminal,
  fitTerminal,
  getTerminal,
  invalidateFitCache,
} from '../../terminal/registry';
import { surfaceId } from '../../types/model';

interface Props {
  sessionId: string;
}

/**
 * pty://exit の購読はサーフェスごとに 1 度だけ確立し、EditorSurface の
 * mount/unmount をまたいで生かす。
 *
 * F3（PR 単位レビュー）: 従来はこの購読を effect の cleanup で毎回 unlisten して
 * いたため、アンマウント中（Cmd+2 などで画面を離れている間）に nvim が終了すると
 * イベントを取りこぼしていた。`editorSurfaces` は永続化されないグローバルストア
 * なので、取りこぼすと `live` のまま固まり、再訪しても再 spawn されず（"登録済み"
 * ガード）、再起動オーバーレイにも到達できなくなる。
 *
 * ptyBridge.ensurePtySubscription の「登録は 1 回、以後は生存させる」設計に揃え、
 * ここでも sid ごとに 1 度だけ登録して以後は解除しない。Rust 側は世代の古い
 * pty://exit をフロントへ転送しない（src-tauri/src/pty/mod.rs の
 * RegistrySink::on_exit、still_current 判定）ため、D4 の再起動（同じ sid で
 * spawn_editor をやり直す）を跨いでも誤発火しない。
 */
const liveExitSubscriptions = new Set<string>();

/** テスト専用。モジュールスコープの購読状態を初期化する。 */
export function resetEditorExitSubscriptionsForTest(): void {
  liveExitSubscriptions.clear();
}

/**
 * コンテナが実寸を持つときだけ fit して PTY へ反映する（設計判断 D7）。
 * 呼び出し元は 3 箇所あり、resize_pty の成否はそれぞれ異なる:
 *   1. 購読解決直後（spawn_editor より前）— PTY がまだ存在せず NotFound で reject する
 *   2. spawn_editor 成功直後（PR 24 / #56 の fix）— PTY が存在するので成功する。
 *      xterm の実寸を初めて PTY へ届けるのがこの呼び出しである
 *   3. ResizeObserver 発火時 — マウント中ずっと張られているので、PTY 終了後に
 *      リサイズされると再び NotFound で reject しうる
 * 1 と 3 は「まだ存在しない / もう存在しない PTY への通知」なので失敗しても
 * 何も壊れない。resizePty の catch はこれら全経路に共通の握り潰しであり、
 * 2 の成功を妨げるものではない（void のままだと開くたびに unhandled rejection が出る）。
 * registry.ts の ackPty / writePty、TerminalPane の syncSize と同じ理由。
 */
function fitAndResize(sid: string, container: HTMLElement | null): void {
  if (!container || container.offsetWidth === 0 || container.offsetHeight === 0) return;
  const size = fitTerminal(sid);
  if (size === null) return;
  resizePty(sid, size.cols, size.rows).catch(() => {
    // 上記の理由により意図的に無視する
  });
}

/**
 * 1 セッション分の nvim ペイン。
 * マウント時に遅延起動し（設計判断 D2）、アンマウント時は detachTerminal するだけで PTY は生かす（D3）。
 * disposeTerminal はここでは絶対に呼ばない（契約 §16。呼ぶのは EditorView の再起動経路だけ）。
 * term.onData も接続しない（契約 §16 / D12。ensureTerminal が唯一の接続点）。
 */
export function EditorSurface({ sessionId }: Props): JSX.Element {
  const containerRef = useRef<HTMLDivElement | null>(null);
  // 契約 §16 / §11.4.6: フォーカスは呼び出し側の責務であり、modal === null のときにのみ
  // 当てる。「マウント時に 1 度評価する」ではなく modal の遷移に追従する必要があるため、
  // ここで購読してフォーカス専用 effect の依存にする（下の第 2 effect）
  const modal = useAppStore((s) => s.modal);
  // フォーカスは attachTerminal の**後**でしか効かない（それまで host は display:none で、
  // xterm の textarea も term.open() されるまで存在しない）。EditorSurface の attach は
  // 購読の解決を待つぶん effect の同期部分より後ろにずれるので、その完了を状態にして
  // フォーカス effect の依存に入れる。TerminalPane は attach が同期なのでこれが要らない
  const [attached, setAttached] = useState(false);

  useEffect(() => {
    const sid = surfaceId(sessionId, 'editor');
    let cancelled = false;
    setAttached(false);

    const run = async () => {
      // ensurePtySubscription も内部で ensureTerminal を呼ぶが、出力が届く前に
      // macOptionIsMeta を効かせるため先に適用しておく。
      // scrollback は ensureTerminal が :editor 接尾辞から自動適用する（契約 §19）
      const term = ensureTerminal(sid);
      applyEditorTerminalOptions(term);

      // 再起動オーバーレイ（D4）用の終了購読。ptyBridge の ensurePtySubscription は
      // ハンドラ引数を取らないため、終了の通知はここで別に受ける。
      // pty://exit は 1 回しか発火せず、ハンドラもストアを更新するだけなので、
      // 契約 §16 が禁じている「出力の二重購読」「打鍵の二重送信」にはあたらない。
      // 購読自体は sid ごとに 1 度だけ（F3。上の liveExitSubscriptions のコメント参照）。
      // 既に生存中ならここでは何もしない
      if (!liveExitSubscriptions.has(sid)) {
        const un = await onPtyExit(sid, (payload) => {
          // editor PTY の終了は runtime_state を変えない（契約 §2。Rust 側は M2-1 の
          // RuntimeSender::note_surface が弾き、Task 8 がその不変条件を固定している）。
          // ここは再起動 UI を出すためだけに使う
          useAppStore
            .getState()
            .setEditorSurface(sessionId, { kind: 'exited', exitCode: payload.exit_code });
        });
        if (cancelled) {
          // StrictMode の setup → cleanup → setup で、この setup が中断された側
          // （2 回目の setup が生き残る）。ここは「生存中」に数えていないので、
          // 中断された自分の登録だけを解除する
          un();
          return;
        }
        liveExitSubscriptions.add(sid);
      }

      // ★契約 §16: pty://data の購読が完了するまで spawn_editor を投げない。
      // 待たないと nvim の初回の全画面描画がリスナ不在で捨てられ、
      // 画面が真っ黒のまま「キーを押すと突然描画される」状態になる
      await ensurePtySubscription(sid);
      if (cancelled) return;

      const container = containerRef.current;
      if (container) {
        attachTerminal(sid, container);
        fitAndResize(sid, container);
        // ★ term.focus() はここでは呼ばない（契約 §16 / §11.4.6）。
        //   フォーカスは下の「フォーカス専用 effect」が modal の遷移に追従して当てる。
        //   ここでは「当てられる状態になった」ことだけを伝える
        setAttached(true);
      }

      // 起動済み / 起動中 / エラー確定なら spawn しない（リトライループを作らない）。
      // 起動状態は editorSurfaces（Task 6）へ一本化する —— ptyBridge の
      // isStarted / markStarted を使わないのは、spawn_editor が契約 §19 で冪等と
      // 確定しており二重 invoke が無害だからである。
      //
      // ★ この読み取りと spawning の書き込みの間に await を挟まないこと。
      //   挟むと 2 つの run が同時に通過しうる。逆に、この 2 行を最初の await より
      //   **前**へ動かしてもいけない —— React 18 StrictMode は setup → cleanup →
      //   setup を同一コミット内で走らせるので、2 回目の setup が 1 回目の書いた
      //   spawning を見て自分を止め、**spawn_editor が 1 度も飛ばなくなる**
      //   （EditorSurface.test.tsx の StrictMode のテストがこれを固定している）。
      //   TerminalPane が isStarted の check と markStarted の set を await の後の
      //   同一同期ブロックで行っているのと同じ構造である
      const store = useAppStore.getState();
      if (store.editorSurfaces[sessionId] !== undefined) return;
      store.setEditorSurface(sessionId, { kind: 'spawning' });

      // ★ ここには cancelled ガードを置かない（fix round 1・Important 1）。
      //   cancelled が守るのは「アンマウント済みコンポーネントの DOM 操作」であって、
      //   editorSurfaces は sessionId をキーにしたグローバルストアであり
      //   コンポーネントの寿命とは無関係である。in-flight 中に Cmd+2 で画面を離れると、
      //   抑止した場合は spawning のまま固まる —— 再び Cmd+3 しても上の「登録済み」
      //   ガードで再試行されず、starting にはオーバーレイが無いのでエラー表示も
      //   再試行ボタンも出ない。editorSurfaces は永続化されないため、アプリを
      //   再起動する以外に復旧手段が無くなる
      try {
        await spawnEditor(sessionId);
        // PR 24 / #56 の人間ゲートで発見: spawn_editor は固定 80x24 で PTY を作る
        // （src-tauri/src/pty/editor.rs）。起動前に飛んだ resize_pty は PTY 不在で
        // NotFound になり握り潰されるため、xterm の実寸が PTY に一度も届かない。
        // fitTerminal は直近サイズキャッシュ（entry.lastSize）と同寸なら null を返して
        // resize_pty を飛ばすので、キャッシュを無効化してから測り直す
        // （TerminalPane.tsx の start_session 解決後と同じ形。registry.ts のドキュメント
        // コメントが同じ失敗形を記録している）。
        invalidateFitCache(sid);
        fitAndResize(sid, containerRef.current);
        // exited を上書きしないための epoch ガードは意図的に置いていない。
        // pty://exit は spawn_editor が生成した子プロセスの終了後に emit されるため、
        // その発生はこの spawnEditor 呼び出しの応答より厳密に後であり、exited が
        // 先に書かれる経路が無い(Rust 側 src-tauri/src/pty/editor.rs の
        // EditorSpawnPlan::Spawn の返却地点に対になる注記がある)。
        // spawn_editor が子プロセス起動の直後に返らなくなったら、この前提は崩れる。
        useAppStore.getState().setEditorSurface(sessionId, { kind: 'live' });
      } catch (e) {
        useAppStore
          .getState()
          .setEditorSurface(sessionId, { kind: 'error', message: toAppError(e).message });
      }
    };

    void run();

    // ResizeObserver はサイズが変化したときにしか発火しない（ポーリングではない）
    const observer = new ResizeObserver(() => {
      fitAndResize(sid, containerRef.current);
    });
    if (containerRef.current) observer.observe(containerRef.current);

    return () => {
      cancelled = true;
      observer.disconnect();
      // F3: pty://exit の購読はここでは外さない。detachTerminal は DOM から
      // 切り離すだけで PTY 自体・pty://data の購読（ptyBridge）は生かしたままなので、
      // アンマウント中に nvim が終了しても liveExitSubscriptions に登録済みの
      // ハンドラが exited をストアへ書き続ける。再訪したとき editorSurfaces が
      // exited のまま残るので、EditorView の再起動オーバーレイに到達できる
      // （購読を切っていた旧実装ではここで取りこぼしていた）
      detachTerminal(sid);
    };
  }, [sessionId]);

  // 契約 §16 / §11.4.6: フォーカスは modal === null のときにのみ当てる。上の attach
  // effect とは**別 effect**にして依存に modal を含める ——「マウント時に 1 度評価する」
  // のではなく modal の遷移そのものに追従させるため。モーダルを閉じた瞬間（非 null →
  // null）にも、この EditorSurface が再マウントされることなくフォーカスを戻す必要がある。
  // attach effect の依存に modal を混ぜてはならない（モーダルの開閉のたびに
  // detach → attach が走り、契約 §16 が定めた detach/attach の用途から外れる）。
  // nvim はフォーカスが来ないと一切操作できない。
  // 実装の形は src/views/TerminalView/TerminalPane.tsx の第 2 effect に合わせてある
  // （attached の段だけが追加分。上の useState のコメントを参照）
  useEffect(() => {
    if (!attached || modal !== null) return;
    getTerminal(surfaceId(sessionId, 'editor'))?.focus();
  }, [sessionId, modal, attached]);

  return <div ref={containerRef} className="editor-surface" />;
}
