import { invoke } from '@tauri-apps/api/core';

import type {
  CliKind,
  KanbanStatus,
  Project,
  Session,
  SessionMode,
  SessionPatch,
} from '../types/model';

// invoke を呼んでよいのはこのファイルだけ。コンポーネントもストアもここを経由する。

export const createProject = (
  name: string,
  repoPath: string,
  defaultCli: CliKind,
): Promise<Project> => invoke('create_project', { name, repoPath, defaultCli });

export const listProjects = (): Promise<Project[]> => invoke('list_projects');

export interface CreateSessionArgs {
  projectId: string;
  title: string;
  description: string;
  mode: SessionMode;
  branch: string | null;
  cliKind: CliKind;
  cliCommand: string | null;
}

export const createSession = (args: CreateSessionArgs): Promise<Session> =>
  invoke('create_session', { ...args });

export const updateSession = (id: string, patch: SessionPatch): Promise<Session> =>
  invoke('update_session', { id, patch });

export const listSessions = (projectId: string, includeArchived: boolean): Promise<Session[]> =>
  invoke('list_sessions', { projectId, includeArchived });

export const moveSession = (
  id: string,
  toStatus: KanbanStatus,
  toIndex: number,
): Promise<Session[]> => invoke('move_session', { id, toStatus, toIndex });

export const startSession = (id: string): Promise<Session> => invoke('start_session', { id });

export const stopSession = (id: string): Promise<Session> => invoke('stop_session', { id });

/** ブランチ名の提案。衝突していれば空いている候補が返る。ユーザーは編集できる。
 *  契約 §60.1.1: 既に id を持つセッション専用。新規作成ダイアログからは呼べない。 */
export const suggestBranchName = (
  projectId: string,
  title: string,
  sessionId: string,
): Promise<string> => invoke('suggest_branch_name', { projectId, title, sessionId });

export const writePty = (surfaceId: string, data: string): Promise<void> =>
  invoke('write_pty', { surfaceId, data });

export const writePtyBytes = (surfaceId: string, base64: string): Promise<void> =>
  invoke('write_pty_bytes', { surfaceId, base64 });

export const resizePty = (surfaceId: string, cols: number, rows: number): Promise<void> =>
  invoke('resize_pty', { surfaceId, cols, rows });

export const ackPty = (surfaceId: string, seq: number): Promise<void> =>
  invoke('ack_pty', { surfaceId, seq });

/** nvim 用 PTY を遅延起動し surface_id を返す。既に起動済みなら同じ値を返す。 */
export const spawnEditor = (sessionId: string): Promise<string> =>
  invoke('spawn_editor', { sessionId });

export type HookLiveness = 'not_applicable' | 'pending' | 'healthy' | 'unreachable';

export interface SessionHookStatus {
  session_id: string;
  cli_kind: CliKind;
  liveness: HookLiveness;
  last_hook_at: number | null;
  heuristics_active: boolean;
}

export interface HooksDiagnostics {
  socket_path: string;
  listener_alive: boolean;
  sessions: SessionHookStatus[];
}

/** 設定画面向けの hooks 疎通ステータス。定期リフレッシュはしない
 *  （パネルを開いたときと session://state を受けたときにだけ呼ぶ）。 */
export const getHooksDiagnostics = (): Promise<HooksDiagnostics> =>
  invoke<HooksDiagnostics>('get_hooks_diagnostics');
