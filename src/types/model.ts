// 契約 §2 / §4 / §6: src-tauri/src/model.rs と 1:1 対応させる。
// フィールド名は Rust 側の serde 表現（snake_case）をそのまま使う。

export type KanbanStatus = 'backlog' | 'in_progress' | 'review' | 'done';
export type SessionMode = 'worktree' | 'in_place';
export type CliKind = 'claude' | 'codex' | 'shell' | 'custom';
export type RuntimeState =
  'running' | 'waiting_input' | 'idle' | 'exited' | 'interrupted' | 'error'; // 契約 §2 の 6 値
export type SurfaceKind = 'agent' | 'editor';
/** 契約 §28.1。'split2' = 左右分割 / 'split2-v' = 上下分割。 */
export type Layout = 'single' | 'split2' | 'split2-v';

/** カンバンの列の表示順。DB の並び順ではなくこれが表示の正典。 */
export const KANBAN_STATUSES: KanbanStatus[] = ['backlog', 'in_progress', 'review', 'done'];

export interface Project {
  id: string;
  name: string;
  repo_path: string;
  default_cli: CliKind;
  created_at: number;
  updated_at: number;
}

// 契約 §34.3: last_runtime_error の直後、archived_at の直前に first_started_at を置く（18 フィールド）。
// 契約 §20（M3-3）: heuristics_enabled / silence_timeout_secs は first_started_at の後、archived_at の前
// （src-tauri/src/model.rs の pub struct Session と同じ位置）。
export interface Session {
  id: string;
  project_id: string;
  title: string;
  description: string;
  kanban_status: KanbanStatus;
  sort_order: number;
  mode: SessionMode;
  branch: string | null;
  worktree_path: string | null;
  cli_kind: CliKind;
  cli_command: string | null;
  claude_session_id: string | null;
  last_runtime_state: RuntimeState;
  last_runtime_error: string | null;
  first_started_at: number | null;
  heuristics_enabled: boolean;
  silence_timeout_secs: number;
  archived_at: number | null;
  created_at: number;
  updated_at: number;
}

/**
 * 契約 §7 の SessionPatch。
 * キーが無い = 変更しない / archived_at: null = アーカイブ解除 / archived_at: 数値 = アーカイブ。
 * Tauri の camelCase 変換はコマンド引数名にしか効かないため、ここは snake_case のまま送る。
 */
export interface SessionPatch {
  title?: string;
  description?: string;
  kanban_status?: KanbanStatus;
  sort_order?: number;
  archived_at?: number | null;
  // 契約 §20（M3-3）: lane-controller の解釈で追加。計画は interface Session のみを指示しているが、
  // Rust の SessionPatch（src-tauri/src/model.rs）に同じ Task 10 で 2 フィールドが入ったため揃えた。
  heuristics_enabled?: boolean;
  silence_timeout_secs?: number;
  /** クリア専用。null 以外を渡すと invalid_state で拒否される */
  claude_session_id?: null;
}

/** worktree の作業ツリー状態（契約 §7.2）。`worktree_status` コマンドの戻り値。 */
export interface WorktreeStatus {
  dirty: boolean;
  /** `git status --porcelain` の行そのまま（例: '?? new.txt', ' M src/a.rs'） */
  entries: string[];
}

export type AppErrorCode =
  'db' | 'not_found' | 'pty_spawn' | 'git' | 'cli_not_found' | 'invalid_state' | 'io';

export interface AppError {
  code: AppErrorCode;
  message: string;
}

/** 契約 §5 */
export const surfaceId = (sessionId: string, kind: SurfaceKind) => `${sessionId}:${kind}`;

// 契約 §23 / 計画第1部 §2: src-tauri/src/session/cli_args.rs の ResumePlan / FreshStartReason と 1:1。
export type FreshStartReason = 'ambiguous_in_place_conversation' | 'no_conversation_restore';
export type ResumePlan =
  | { kind: 'claude_resume'; claude_session_id: string }
  | { kind: 'claude_continue' }
  | { kind: 'fresh_start'; reason: FreshStartReason };

/**
 * `focus://session/{session_id}`（契約 §8）のペイロード。
 * TS 側の置き場所の正典は src/types/model.ts（契約 §33.2。Rust 側の model.rs と 1:1）。
 * emit するのは M2-3（通知クリック）。listen ラッパの正典は src/ipc/events.ts の listenFocus（契約 §35）。
 */
export interface FocusPayload {
  session_id: string;
  surface_kind: SurfaceKind;
}

// 契約 §33.2 の正典をそのまま写す。§8 の 13 バリアントと 1:1。
// 後続フェーズ（M2-4 / M3-3）はこの union に値を足さない
export type StateReason =
  | 'spawned'
  | 'hook_notification'
  | 'hook_stop'
  | 'pty_exited'
  | 'startup_normalize'
  | 'bel_detected'
  | 'silence_timeout'
  | 'user_stopped'
  | 'output_activity'
  | 'user_input'
  | 'hook_permission'
  | 'resume_failed'
  | 'spawn_failed';

/** `session://state/{session_id}`（契約 §8）のペイロード。 */
export interface SessionStatePayload {
  session_id: string;
  runtime_state: RuntimeState;
  reason: StateReason;
}

/**
 * macOS の通知許可状態（契約 §83.3）。`notification_permission` コマンドの戻り値。
 * `#[tauri::command]` の返り値型なので TS 側の置き場所は常に src/types/model.ts（契約 §83.3.1）。
 */
export type NotifyPermission = 'unknown' | 'granted' | 'denied';

/**
 * フロントの表示中ビュー（契約 §7.2 / `src-tauri/src/notify/policy.rs` の `ViewKind` と1:1）。
 * `set_visibility_context` コマンドの引数型なので TS 側の置き場所は常に src/types/model.ts（契約 §83.3.1）。
 * 型は実行時に消えるためテストから触れない。VIEW_KINDS を唯一の源とし、ViewKind はそこから導出する
 * （`AppStore['view']` 側で別の union を手書きしない）。
 */
export const VIEW_KINDS = ['kanban', 'terminal', 'editor'] as const;
export type ViewKind = (typeof VIEW_KINDS)[number];
