// 契約 §2 / §4 / §6: src-tauri/src/model.rs と 1:1 対応させる。
// フィールド名は Rust 側の serde 表現（snake_case）をそのまま使う。

export type KanbanStatus = 'backlog' | 'in_progress' | 'review' | 'done';
export type SessionMode = 'worktree' | 'in_place';
export type CliKind = 'claude' | 'codex' | 'shell' | 'custom';
export type RuntimeState =
  'running' | 'waiting_input' | 'idle' | 'exited' | 'interrupted' | 'error'; // 契約 §2 の 6 値
export type SurfaceKind = 'agent' | 'editor';

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
}

export type AppErrorCode =
  'db' | 'not_found' | 'pty_spawn' | 'git' | 'cli_not_found' | 'invalid_state' | 'io';

export interface AppError {
  code: AppErrorCode;
  message: string;
}

/** 契約 §5 */
export const surfaceId = (sessionId: string, kind: SurfaceKind) => `${sessionId}:${kind}`;

/**
 * `focus://session/{session_id}`（契約 §8）のペイロード。
 * TS 側の置き場所の正典は src/types/model.ts（契約 §33.2。Rust 側の model.rs と 1:1）。
 * emit するのは M2-3（通知クリック）。listen ラッパの正典は src/ipc/events.ts の listenFocus（契約 §35）。
 */
export interface FocusPayload {
  session_id: string;
  surface_kind: SurfaceKind;
}
