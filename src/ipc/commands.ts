import { invoke } from '@tauri-apps/api/core';

import type { AppStore } from '../store';
import type {
  CliKind,
  KanbanStatus,
  NotifyPermission,
  Project,
  Session,
  SessionMode,
  SessionPatch,
  WorktreeStatus,
} from '../types/model';

// invoke を呼んでよいのはこのファイルだけ。コンポーネントもストアもここを経由する。

export const createProject = (
  name: string,
  repoPath: string,
  defaultCli: CliKind,
): Promise<Project> => invoke('create_project', { name, repoPath, defaultCli });

export const listProjects = (): Promise<Project[]> => invoke('list_projects');

/**
 * プロジェクトを削除する（契約 §44.2 / §130.4）。`sessions` は §3 の ON DELETE CASCADE で
 * 一緒に消えるので、呼ぶ前に対象プロジェクトの全セッションへ `stop_session` を回すこと
 * （行だけが消えて PTY が生き残ると、どのカードからも辿れない孤児になる）。
 * worktree は消さない —— 消す導線は 🧹（`cleanup_worktree`）が持つ。
 */
export const deleteProject = (id: string): Promise<void> => invoke('delete_project', { id });

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

export const resumeSession = (id: string): Promise<Session> => invoke('resume_session', { id });

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
 *  （パネルと編集ダイアログを開いたとき = マウント時に 1 回だけ呼ぶ。
 *  `session://state` での再取得は未実装）。 */
export const getHooksDiagnostics = (): Promise<HooksDiagnostics> =>
  invoke<HooksDiagnostics>('get_hooks_diagnostics');

/** 表示中のビューとセッションを Rust に伝える（前面時の通知抑制に使う）。 */
export const setVisibilityContext = (
  view: AppStore['view'],
  visibleSessionIds: string[],
): Promise<void> => invoke('set_visibility_context', { view, visibleSessionIds });

/** 現在の通知許可状態を読む（プロンプトは出さない）。 */
export const notificationPermission = (): Promise<NotifyPermission> =>
  invoke('notification_permission');

/** macOS のシステム設定「通知」ペインを開く。 */
export const openNotificationSettings = (): Promise<void> => invoke('open_notification_settings');

/** worktree の作業ツリー状態を読む（契約 §7.2）。 */
export const worktreeStatus = (sessionId: string): Promise<WorktreeStatus> =>
  invoke<WorktreeStatus>('worktree_status', { sessionId });

/** worktree を破棄する。dirty な場合は force が要る。 */
export const cleanupWorktree = (sessionId: string, force: boolean): Promise<void> =>
  invoke('cleanup_worktree', { sessionId, force });

/** フロントの初回ペイント完了を Rust へ通知する（契約 §0 の起動時間計測点。M3-4 Task 13）。
 *  `src/App.tsx` が初回ペイント後に 1 回だけ呼ぶ。 */
export const reportFrontendReady = (): Promise<void> => invoke('report_frontend_ready');
