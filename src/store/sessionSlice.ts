import type { StateCreator } from 'zustand';

import {
  createSession,
  listSessions,
  moveSession,
  resumeSession as resumeSessionCmd,
  updateSession as updateSessionCmd,
  type CreateSessionArgs,
} from '../ipc/commands';
import type {
  KanbanStatus,
  RuntimeState,
  Session,
  SessionPatch,
  SessionStatePayload,
  StateReason,
} from '../types/model';
import { emptySessionOrder, indexSessions, moveCardInOrder } from './kanbanOrder';
import type { AppStore } from './index';
import { toAppError } from './uiSlice';

export { emptySessionOrder, indexSessions } from './kanbanOrder';

/**
 * await をまたぐ経路の先頭で呼び、戻り値の関数を「応答を適用する直前」に呼んで判定する。
 * sessions / sessionOrder は常に activeProjectId のものである、という不変条件
 * （loadSessions が最初に宣言したもの。上のコメント参照）を、addSession / editSession /
 * archiveSession / moveCard の成功・失敗の両経路でも守るための共通ガード。
 * IPC 往復中にプロジェクトが切り替わっていたら、応答は盤面へ適用せず捨てる
 * （呼び出し元への返り値・throw はこのガードと無関係に行うこと）。
 */
function isStillActiveProject(get: () => AppStore): () => boolean {
  const projectId = get().activeProjectId;
  return () => get().activeProjectId === projectId;
}

export interface SessionSlice {
  sessions: Record<string, Session>;
  sessionOrder: Record<KanbanStatus, string[]>;

  /** DB の last_runtime_state とは別管理（契約 §10）。真実は PTY/hooks 由来。 */
  runtimeStates: Record<string, RuntimeState>;
  /** ツールチップとデバッグ用（契約 §8）。購読者は RuntimeBadge のみ。 */
  runtimeReasons: Record<string, StateReason>;
  /**
   * runtime_state === 'error' のときだけ入る生 stderr（契約 §42.3）。
   * DB のミラーである sessions[id].last_runtime_error とは別管理にする ——
   * 失敗時はコマンドが Err を返すので sessions は更新されず、
   * かつ applyStateEvent は sessions を参照ごと変更してはならない。
   * 購読してよいのは KanbanCardError だけ（契約 §38.3 の許可リスト）。
   */
  runtimeErrors: Record<string, string>;
  /**
   * reason: 'resume_failed'（契約 §8 の StateReason）を受け取ったセッションの id 集合。
   * InterruptedOverlay / KanbanCardResume の「新しい会話として開始」導線と
   * retryResumeAsFresh はこの配列だけを見る（第1部 §4.4）。
   */
  resumeFailedSessionIds: string[];

  loadSessions: (projectId: string) => Promise<void>;
  addSession: (args: CreateSessionArgs) => Promise<Session>;
  moveCard: (sessionId: string, to: KanbanStatus, index: number) => Promise<void>;
  editSession: (id: string, patch: SessionPatch) => Promise<Session>;
  archiveSession: (id: string) => Promise<void>;
  applyStateEvent: (p: SessionStatePayload) => void;
  /**
   * last_runtime_state から初期値を埋める。
   * reset=false（既定）は「未知のキーだけ埋める」。start_session の戻り値で呼んでも
   * 先に届いた実時間の値を潰さない。reset=true はプロジェクト切替時に総入れ替えする。
   */
  seedRuntimeStates: (sessions: Session[], reset?: boolean) => void;
  /**
   * start_session / resume_session の catch から setError と同じ場所で呼ぶ（契約 §42.3 規約 4）。
   * 渡すのは AppError の message —— mark_error が DB へ書くのと同一の文字列である。
   * 許可リスト（契約 §40.3）の複製はしない。ズレの境界は契約 §42.3.1 が定めている。
   */
  setRuntimeError: (sessionId: string, message: string) => void;
  /** カードの再開ボタン / InterruptedOverlay が呼ぶ（第1部 §4.4: 経路を分けない）。 */
  resumeSession: (sessionId: string) => Promise<void>;
  /**
   * 無効な claude_session_id を捨ててから再開する（第1部 §4.2）。
   * SessionPatch は claude_session_id のクリアだけを受け付ける（§4.9）。
   */
  retryResumeAsFresh: (sessionId: string) => Promise<void>;
}

export const createSessionSlice: StateCreator<AppStore, [], [], SessionSlice> = (set, get) => ({
  sessions: {},
  sessionOrder: emptySessionOrder(),
  runtimeStates: {},
  runtimeReasons: {},
  runtimeErrors: {},
  resumeFailedSessionIds: [],

  loadSessions: async (projectId) => {
    // アーカイブ済みは表示しない（復活 UX は M3-4）
    const list = await listSessions(projectId, false);
    // 応答が返るまでにプロジェクトが切り替わっていたら捨てる（M1-1 からの申し送り）。
    // sessions / sessionOrder を所有する sessionSlice が「これらは常に activeProjectId の
    // ものである」という不変条件も持つ。projectSlice.setActiveProject は
    // activeProjectId を set してから loadSessions を await するので、初回ロードは弾かれない。
    // 【この行より上で set() しないこと】ガードは「この応答を捨てるかどうか」の判定であり、
    // ガードより前に set すると、捨てたはずの応答の副作用だけがストアに残る。
    // 例: 契約 §34.6 で M2-1 が足す予定の seedRuntimeStates(list, true) は、必ずこのガードの
    // 下（return の後）に置くこと。上に置くと runtimeStates だけ stale なプロジェクトで
    // seed され、sessions / sessionOrder は現行のまま、という split-brain に戻ってしまう。
    if (get().activeProjectId !== projectId) return;
    set(indexSessions(list));
    // 起動時正規化済みの last_runtime_state から ⏸ を復元する
    get().seedRuntimeStates(list, true);
  },

  addSession: async (args) => {
    const isStillActive = isStillActiveProject(get);
    const created = await createSession(args);
    if (isStillActive()) {
      const { sessions, sessionOrder } = get();
      const column = sessionOrder[created.kanban_status];
      set({
        sessions: { ...sessions, [created.id]: created },
        sessionOrder: { ...sessionOrder, [created.kanban_status]: [...column, created.id] },
      });
    }
    return created;
  },

  moveCard: async (sessionId, to, index) => {
    const isStillActive = isStillActiveProject(get);
    const { sessions, sessionOrder } = get();
    const target = sessions[sessionId];
    if (!target) return;

    // 楽観更新: 配列の並べ替えだけを行う。sort_order の実値は算出しない（契約 §7.4）。
    // DnD の手応えを IPC の往復で待たせないため（判断 3）。
    const nextOrder = moveCardInOrder(sessionOrder, sessionId, to, index);
    set({
      sessions: { ...sessions, [sessionId]: { ...target, kanban_status: to } },
      sessionOrder: nextOrder,
    });

    try {
      // 戻り値は「移動先の列」の全 Session（sort_order 昇順・同値は id タイブレーク。契約 §49.4）。
      // 移動元の列は 1 行も変化しないので返らない。楽観更新で除去済みの状態が正しい。
      const column = await moveSession(sessionId, to, index);
      if (isStillActive()) {
        const merged = { ...get().sessions };
        for (const s of column) merged[s.id] = s;
        set({
          sessions: merged,
          sessionOrder: { ...get().sessionOrder, [to]: column.map((s) => s.id) },
        });
      }
    } catch (e) {
      // DB が受け付けなかった位置にカードを残さない
      // ただし切り替わっていたら、今の(別プロジェクトの)盤面を A の状態で
      // 上書きしてはいけない。throw はガードと無関係に行う
      if (isStillActive()) {
        set({ sessions, sessionOrder });
      }
      throw e;
    }
  },

  editSession: async (id, patch) => {
    // title / description しか変えない想定（判断 10）。並びに影響しないので
    // sessionOrder は触らず、sessions の当該エントリだけを差し替える。
    const isStillActive = isStillActiveProject(get);
    const saved = await updateSessionCmd(id, patch);
    if (isStillActive()) {
      set({ sessions: { ...get().sessions, [saved.id]: saved } });
    }
    return saved;
  },

  archiveSession: async (id) => {
    const isStillActive = isStillActiveProject(get);
    const snapshot = get().sessions;
    const prevOrder = get().sessionOrder;
    const target = snapshot[id];
    if (target === undefined) return;

    // 楽観更新: 盤面からは当該列だけ除去する（buildSessionOrder による全列再構築は
    // moveCard の in-flight 中と重なると、移動中のカードが古い sort_order の位置へ
    // 吸着して見えるおそれがあるため避ける）。
    const archivedAt = Date.now();
    const optimisticSessions = { ...snapshot, [id]: { ...target, archived_at: archivedAt } };
    const optimisticOrder = {
      ...prevOrder,
      [target.kanban_status]: prevOrder[target.kanban_status].filter((sid) => sid !== id),
    };
    set({ sessions: optimisticSessions, sessionOrder: optimisticOrder });

    try {
      const saved = await updateSessionCmd(id, { archived_at: archivedAt });
      if (isStillActive()) {
        set({ sessions: { ...get().sessions, [saved.id]: saved } });
      }
    } catch (e) {
      // 切り替わっていたら、今の(別プロジェクトの)盤面を A の状態で上書きしない。
      // throw はガードと無関係に行う（呼び出し側がエラーを表示する契約は変えない）
      if (isStillActive()) {
        set({ sessions: snapshot, sessionOrder: prevOrder });
      }
      throw e;
    }
  },

  applyStateEvent: (p) =>
    set((s) => {
      // 契約 §42.3 規約 3: error 以外へ遷移したら生 stderr を捨てる。
      // DB 側の §17（set_last_runtime_state が state != Error で NULL に戻す）と同じ規則。
      // error のときは触らない —— メッセージはイベントで来ないので、消すと空になる。
      const dropError = p.runtime_state !== 'error' && s.runtimeErrors[p.session_id] !== undefined;
      // resume_failed（契約 §8 の StateReason）だけを積む。既に積んであれば内容は変わらない
      // ので、恒等ガードのために「実際に変わるか」を先に判定する（新しい配列を無条件に
      // 作ると、他の 3 項が同値でも参照だけが毎回変わってしまう）。
      const resumeFailedChanged =
        p.reason === 'resume_failed' && !s.resumeFailedSessionIds.includes(p.session_id);
      if (
        s.runtimeStates[p.session_id] === p.runtime_state &&
        s.runtimeReasons[p.session_id] === p.reason &&
        !dropError &&
        !resumeFailedChanged
      ) {
        // 新しいオブジェクトを作らない = 無関係な購読者を再レンダリングさせない
        return {};
      }
      const runtimeErrors = dropError ? { ...s.runtimeErrors } : s.runtimeErrors;
      if (dropError) delete runtimeErrors[p.session_id];
      const resumeFailedSessionIds = resumeFailedChanged
        ? [...new Set([...s.resumeFailedSessionIds, p.session_id])]
        : s.resumeFailedSessionIds;
      return {
        runtimeStates: { ...s.runtimeStates, [p.session_id]: p.runtime_state },
        runtimeReasons: { ...s.runtimeReasons, [p.session_id]: p.reason },
        runtimeErrors,
        resumeFailedSessionIds,
      };
    }),

  setRuntimeError: (sessionId, message) =>
    set((s) => ({ runtimeErrors: { ...s.runtimeErrors, [sessionId]: message } })),

  resumeSession: async (sessionId) => {
    // 失敗時は生 stderr をストアへ残す（契約 §42.3 規約 4）。
    // カードの kanban-card__error はこれを読む。トーストは呼び出し側が出す。
    let session: Session;
    try {
      session = await resumeSessionCmd(sessionId);
    } catch (e: unknown) {
      get().setRuntimeError(sessionId, toAppError(e).message);
      throw e;
    }
    set((s) => ({
      sessions: { ...s.sessions, [sessionId]: session },
      resumeFailedSessionIds: s.resumeFailedSessionIds.filter((id) => id !== sessionId),
    }));
  },

  // クリア結果は resumeSession の成否によらず先にストアへ反映する。そうしないと
  // 再開が失敗したとき、ストアだけ古い ID を持ち続けてカードのラベルが
  // 「会話を再開」に戻り、DB と食い違う。
  retryResumeAsFresh: async (sessionId) => {
    const cleared = await updateSessionCmd(sessionId, { claude_session_id: null });
    set((s) => ({ sessions: { ...s.sessions, [sessionId]: cleared } }));
    await get().resumeSession(sessionId);
  },

  seedRuntimeStates: (list, reset = false) =>
    set((s) => {
      if (reset) {
        const fresh: Record<string, RuntimeState> = {};
        // 契約 §42.3 規約 1・2: runtimeErrors も同じ規則で作り直す。
        // §40.5 の 'error' 例外は runtimeErrors にも効く —— 効かせないと再起動後に
        // ❌ の枠だけが描かれて中身が空になり、§40.5 が防いだ事故が 1 フィールド隣で再発する。
        const freshErrors: Record<string, string> = {};
        for (const sess of list) {
          // 契約 §40.5: 'error' は first_started_at が null でも必ず seed する。
          // 起動に一度も成功していないセッションの起動が失敗すると first_started_at は
          // null のまま last_runtime_state だけが 'error' になる。ここで除外すると
          // 再起動で ❌ が消え、「痕跡がカードに残る」という error の存在理由（契約 §2）が
          // 最初のユースケースで空振りする。
          if (sess.first_started_at === null && sess.last_runtime_state !== 'error') {
            // 一度も起動していない → seed しない（契約 §34.6）。
            // runtimeStates[id] は undefined のままになり、RuntimeBadge が null を返す。
            // ただし既にエントリがある場合は消さない —— loadSessions のスナップショット読み取りが
            // mark_first_started のコミットより先行したとき、実行中のバッジを消さないため。
            const live = s.runtimeStates[sess.id];
            if (live !== undefined) fresh[sess.id] = live;
            const liveErr = s.runtimeErrors[sess.id];
            if (liveErr !== undefined) freshErrors[sess.id] = liveErr;
            continue;
          }
          fresh[sess.id] = sess.last_runtime_state;
          // null の要素はキーを作らない（契約 §42.3 規約 1）
          if (sess.last_runtime_error !== null) freshErrors[sess.id] = sess.last_runtime_error;
        }
        return { runtimeStates: fresh, runtimeReasons: {}, runtimeErrors: freshErrors };
      }
      // 非 reset には除外を適用しない（契約 §34.6）。呼び出し元は start_session /
      // resume_session の戻り値だけで、渡されるのは「たった今起動した」セッションに限られる。
      // 戻り値の Session は mark_first_started の非同期コミットより前に読まれて
      // first_started_at === null を持ちうるため、ここで除外すると §4.5 の自己修復が壊れる。
      let changed = false;
      const next = { ...s.runtimeStates };
      const nextErrors = { ...s.runtimeErrors };
      for (const sess of list) {
        if (next[sess.id] === undefined) {
          next[sess.id] = sess.last_runtime_state;
          changed = true;
        }
        // 契約 §42.3 規約 1: 非 reset も「未知のキーだけ埋める」。
        if (nextErrors[sess.id] === undefined && sess.last_runtime_error !== null) {
          nextErrors[sess.id] = sess.last_runtime_error;
          changed = true;
        }
      }
      return changed ? { runtimeStates: next, runtimeErrors: nextErrors } : {};
    }),
});

/**
 * ID 集合を安定した文字列に畳んでから返す。
 * `Object.keys(...)` をそのまま selector にすると毎回新しい配列になり、
 * 無関係な set のたびに App 全体が再レンダリングされる。
 */
export const selectSessionIdsKey = (s: { sessions: Record<string, unknown> }): string =>
  Object.keys(s.sessions).sort().join(',');
