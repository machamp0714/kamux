// M2-1 がこのモジュールに SessionManager / runtime_state.rs を追加する
pub mod cli_args;

use tauri::{AppHandle, State};

use crate::error::AppResult;
use crate::model::{Session, SurfaceKind};
use crate::pty::launch_env::LaunchEnv;
use crate::pty::{surface_id, SpawnSpec, DEFAULT_COLS, DEFAULT_ROWS};
use crate::state::AppState;
use cli_args::{build_launch_command, login_shell, resolve_cwd, ResumeMode};

/// `start_session` の `AppHandle` を必要としない部分だけを切り出した純関数。
///
/// 契約 §15 が `PtyManager::spawn(&self, app: &tauri::AppHandle, ..)` を Wry 固定で
/// 凍結しているため、`start_session` 全体は `MockRuntime` の `generate_handler!` に
/// 登録できず IPC テストから到達不能になる（フィックス対象レビュー指摘: Task 8 fix
/// round 1、Important 1）。`id -> get_session -> get_project -> resolve_cwd ->
/// build_launch_command -> SpawnSpec` の組み立て自体は `AppHandle` を要求しない
/// ので、ここへ分離すればランタイム不要の普通のユニットテストで固定できる。
/// `start_session` に残るのは `state.pty.spawn(&app, spec)` の 1 行だけになる。
fn plan_agent_spawn(state: &AppState, id: &str) -> AppResult<(Session, SpawnSpec)> {
    let session = state.store.get_session(id)?;
    let project = state.store.get_project(&session.project_id)?;
    let cwd = resolve_cwd(&session, &project.repo_path);

    // M1-3 は shell のみなので program はログインシェル。
    // M1-4 はここを §18 の resolve_program(binary_name(cli_kind)?) に差し替え、
    // LaunchEnv::from_current_process() を probe_login_env() に差し替える。
    // shell の腕は launch_env を使わない（$SHELL -l が自分で PATH / LANG を作る、契約 §23）
    let launch_env = LaunchEnv::from_current_process();
    let launch = build_launch_command(
        &session,
        &login_shell(),
        &cwd,
        &launch_env,
        ResumeMode::None,
    )?;

    let spec = SpawnSpec {
        surface_id: surface_id(&session.id, SurfaceKind::Agent),
        program: launch.program.to_string_lossy().into_owned(),
        args: launch.args,
        cwd: launch.cwd,
        // KAMUX_SESSION_ID は build_launch_command が入れている
        env: launch.env,
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
    };

    Ok((session, spec))
}

/// セッションの agent サーフェスを起動する。
/// M1-3 では worktree 準備を行わない（M1-4 が start_session の前段に足す）。
/// サイズは 80x24 で起動し、フロントが attach 直後の fit() → resize_pty で合わせる。
#[tauri::command]
pub async fn start_session(
    state: State<'_, AppState>,
    app: AppHandle,
    id: String,
) -> AppResult<Session> {
    let (session, spec) = plan_agent_spawn(&state, &id)?;
    state.pty.spawn(&app, spec)?;
    Ok(session)
}

/// agent サーフェスを殺す。runtime_state の遷移は M2-1 が担当する
#[tauri::command]
pub async fn stop_session(state: State<'_, AppState>, id: String) -> AppResult<Session> {
    let session = state.store.get_session(&id)?;
    state
        .pty
        .kill(&surface_id(&session.id, SurfaceKind::Agent))?;
    Ok(session)
}
