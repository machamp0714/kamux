// モジュールはすべて pub mod で宣言する。private mod にすると dead_code が消えない（§45.1 の実測）
pub mod error;
pub mod model;
pub mod pty;
pub mod session;
pub mod state;
pub mod store;

use std::sync::Arc;

use tauri::{Manager, State, WindowEvent};

use crate::error::AppResult;
use crate::model::{CliKind, KanbanStatus, Project, Session, SessionMode, SessionPatch};
use crate::state::AppState;
use crate::store::{db_path, now_ms, Store};

// 各コマンドは Store への薄いラッパに徹する。ロジックは DAO 側に置く。
// 契約 §7 が async fn を要求する一方、Store の MutexGuard は !Send なので、
// コマンド本体に .await を書いてはならない（ガードが await を跨ぐとコンパイルできない）。
//
// 契約 §17: DAO 名とコマンド名は別物。コマンド create_project -> DAO insert_project。
// get_project と set_* 系はコマンドとして公開しない（Rust 内部専用）。

#[tauri::command]
async fn create_project(
    state: State<'_, AppState>,
    name: String,
    repo_path: String,
    default_cli: CliKind,
) -> AppResult<Project> {
    state.store.insert_project(&name, &repo_path, default_cli)
}

#[tauri::command]
async fn list_projects(state: State<'_, AppState>) -> AppResult<Vec<Project>> {
    state.store.list_projects()
}

// 引数 7 個は契約 §7 のコマンドシグネチャそのもの。まとめると
// TS 側の camelCase キー（projectId/title/description/mode/branch/cliKind/cliCommand）との
// 1:1 対応が崩れるため、分割せずそのまま許可する。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn create_session(
    state: State<'_, AppState>,
    project_id: String,
    title: String,
    description: String,
    mode: SessionMode,
    branch: Option<String>,
    cli_kind: CliKind,
    cli_command: Option<String>,
) -> AppResult<Session> {
    // 採番 -> 構築 -> 挿入。next_sort_order と insert_session はロックを別々に取るので
    // 厳密には原子的ではないが、sort_order に一意制約は無く、同値なら id で
    // タイブレークされるだけなので実害がない（第1部 判断 11）。
    let sort_order = state
        .store
        .next_sort_order(&project_id, KanbanStatus::Backlog)?;
    let session = Session::new_backlog(
        &project_id,
        &title,
        &description,
        mode,
        branch,
        cli_kind,
        cli_command,
        sort_order,
        now_ms(),
    );
    state.store.insert_session(&session)
}

#[tauri::command]
async fn update_session(
    state: State<'_, AppState>,
    id: String,
    patch: SessionPatch,
) -> AppResult<Session> {
    state.store.update_session(&id, &patch)
}

#[tauri::command]
async fn list_sessions(
    state: State<'_, AppState>,
    project_id: String,
    include_archived: bool,
) -> AppResult<Vec<Session>> {
    state.store.list_sessions(&project_id, include_archived)
}

// 契約 §7.1 / §44.1: delete_project は M1-1 が DAO ごと実装して登録する。
// M1-1 に呼び出し側（TS ラッパ・UI）は無い(契約 §44.3。レビューで欠陥として指摘しないこと）。
#[tauri::command]
async fn delete_project(state: State<'_, AppState>, id: String) -> AppResult<()> {
    state.store.delete_project(&id)
}

// カンバン DnD 専用（契約 §7.1 / §7.4 / §49.1）。sort_order の採番をサーバ側で原子的に行う。
#[tauri::command]
async fn move_session(
    state: State<'_, AppState>,
    id: String,
    to_status: KanbanStatus,
    to_index: usize,
) -> AppResult<Vec<Session>> {
    state.store.move_session(&id, to_status, to_index)
}

// 契約 §45.2: tauri::Builder の組み立てとコマンド登録は lib.rs の run() の中だけに置く。
// main.rs は `fn main() { kamux::run() }` の 3 行で固定であり、以後どの計画も編集しない。
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // 契約 §17: db_path() は環境変数 KAMUX_DB_PATH で上書き可
            let store = Arc::new(Store::open(&db_path()?)?);
            app.manage(AppState {
                store,
                pty: pty::PtyManager::new(),
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            // アプリ終了時に PTY の子プロセスを残さない。
            // Cmd+Q でこれが走らない場合は Builder::build() + RunEvent::Exit に移す（第1部 2.6）
            if matches!(event, WindowEvent::Destroyed) {
                if let Some(state) = window.try_state::<AppState>() {
                    for id in state.pty.live_surfaces() {
                        let _ = state.pty.kill(&id);
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            create_project,
            list_projects,
            create_session,
            update_session,
            list_sessions,
            delete_project,
            move_session,
            pty::commands::write_pty,
            pty::commands::write_pty_bytes,
            pty::commands::resize_pty,
            pty::commands::ack_pty,
            session::start_session,
            session::stop_session,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run kamux");
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::error::AppError;
    use crate::model::{CliKind, KanbanStatus, Session, SessionMode};
    use crate::state::AppState;
    use crate::store::{now_ms, test_support::open_temp};

    // Tauri の State<'_, AppState> はユニットテストから構築できないため、
    // コマンドが呼ぶのと同じ経路（AppState -> Store）を直接叩いて配線を検証する。
    // コマンド自体が invoke_handler に登録できることは cargo build が保証する。
    //
    // project / session を 2 件以上作って絞り込みと採番の弁別力を持たせる
    // （1 件しか無いと WHERE 句を落としても緑になる。lane controller 申し送り 6）。
    #[test]
    fn app_state_exposes_the_full_m1_1_crud_path() {
        let (_dir, store) = open_temp();
        let state = AppState {
            store: Arc::new(store),
            pty: crate::pty::PtyManager::new(),
        };

        let project_a = state
            .store
            .insert_project("kamux", "/Users/x/repo/kamux", CliKind::Claude)
            .expect("insert_project");
        let project_b = state
            .store
            .insert_project("other", "/Users/x/repo/other", CliKind::Claude)
            .expect("insert_project");
        assert_eq!(state.store.list_projects().expect("list_projects").len(), 2);
        assert_eq!(
            state
                .store
                .get_project(&project_a.id)
                .expect("get_project")
                .name,
            "kamux"
        );
        assert_eq!(
            state
                .store
                .get_project(&project_b.id)
                .expect("get_project")
                .name,
            "other"
        );

        // create_session コマンドと同じ 3 手（採番 -> 構築 -> 挿入）を project_a に 2 件、
        // project_b に 1 件。採番が project 単位であること、一覧が project で絞り込まれる
        // ことの両方を弁別する。
        let sort_order_1 = state
            .store
            .next_sort_order(&project_a.id, KanbanStatus::Backlog)
            .expect("next_sort_order");
        let session_1 = state
            .store
            .insert_session(&Session::new_backlog(
                &project_a.id,
                "fix login",
                "",
                SessionMode::InPlace,
                None,
                CliKind::Claude,
                None,
                sort_order_1,
                now_ms(),
            ))
            .expect("insert_session");
        assert_eq!(session_1.kanban_status, KanbanStatus::Backlog);
        assert_eq!(session_1.sort_order, 1.0);

        let sort_order_2 = state
            .store
            .next_sort_order(&project_a.id, KanbanStatus::Backlog)
            .expect("next_sort_order");
        let session_2 = state
            .store
            .insert_session(&Session::new_backlog(
                &project_a.id,
                "add signup",
                "",
                SessionMode::InPlace,
                None,
                CliKind::Claude,
                None,
                sort_order_2,
                now_ms(),
            ))
            .expect("insert_session");
        assert_eq!(session_2.sort_order, 2.0);

        let sort_order_b = state
            .store
            .next_sort_order(&project_b.id, KanbanStatus::Backlog)
            .expect("next_sort_order");
        assert_eq!(sort_order_b, 1.0, "採番は project 単位でリセットされる");
        let session_b = state
            .store
            .insert_session(&Session::new_backlog(
                &project_b.id,
                "unrelated",
                "",
                SessionMode::InPlace,
                None,
                CliKind::Claude,
                None,
                sort_order_b,
                now_ms(),
            ))
            .expect("insert_session");

        let patch = serde_json::from_str(r#"{"kanban_status":"in_progress"}"#).expect("patch");
        let moved = state
            .store
            .update_session(&session_1.id, &patch)
            .expect("update_session");
        assert_eq!(moved.kanban_status, KanbanStatus::InProgress);

        let listed_a = state
            .store
            .list_sessions(&project_a.id, false)
            .expect("list_sessions");
        assert_eq!(listed_a.len(), 2, "project_a の 2 件だけが返る");
        assert!(listed_a
            .iter()
            .any(|s| s.id == session_1.id && s.kanban_status == KanbanStatus::InProgress));
        assert!(listed_a.iter().any(|s| s.id == session_2.id));
        assert!(
            listed_a.iter().all(|s| s.id != session_b.id),
            "project_b のセッションが混ざってはいけない"
        );

        // delete_project コマンドと同じ経路。戻り値だけでなく読み直しで消えたことを確認する。
        state
            .store
            .delete_project(&project_b.id)
            .expect("delete_project");
        let remaining = state.store.list_projects().expect("list_projects");
        assert_eq!(remaining.len(), 1);
        assert!(remaining.iter().all(|p| p.id != project_b.id));
        match state.store.get_project(&project_b.id) {
            Err(AppError::NotFound(id)) => assert_eq!(id, project_b.id),
            other => panic!("expected AppError::NotFound, got {other:?}"),
        }
    }

    #[test]
    fn app_state_store_is_shareable_across_threads() {
        // M1-3 の PtyManager / M2-1 の SessionManager がバックグラウンドスレッドから
        // Store を触るため、Arc<Store> がスレッドを跨げることを固定する。
        let (_dir, store) = open_temp();
        let state = AppState {
            store: Arc::new(store),
            pty: crate::pty::PtyManager::new(),
        };
        let project = state
            .store
            .insert_project("kamux", "/Users/x/repo/kamux", CliKind::Claude)
            .expect("insert_project");

        let cloned = Arc::clone(&state.store);
        let id = project.id.clone();
        let handle = std::thread::spawn(move || cloned.get_project(&id).expect("get_project").name);

        assert_eq!(handle.join().expect("join"), "kamux");
    }

    // ここから下は IPC 境界を実際に越えるテスト。上のテストは AppState -> Store の経路を
    // 直叩きしているだけでコマンド本体（#[tauri::command] fn）を 1 行も実行していないため、
    // 次の 3 つの変異がどれも緑のまま残ってしまう:
    //   1. create_project(name, repo_path) の引数を入れ替えてもコンパイルが通る（両方 &str）
    //   2. list_sessions の include_archived を無視して false 固定にしても緑
    //   3. create_session の sort_order を next_sort_order から取らず定数にしても緑
    // JS が実際に送るのと同じ camelCase キーの JSON を tauri::test::get_ipc_response で
    // 投げることで、コマンド本体の引数バインディングそのものを検証する。
    mod ipc {
        use std::sync::Arc;

        use serde_json::{json, Value};
        use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, MockRuntime};
        use tauri::{ipc::CallbackFn, webview::InvokeRequest, Manager, WebviewWindowBuilder};

        use crate::state::AppState;
        use crate::store::test_support::open_temp;

        fn build_app(store: crate::store::Store) -> tauri::App<MockRuntime> {
            mock_builder()
                .manage(AppState {
                    store: Arc::new(store),
                    pty: crate::pty::PtyManager::new(),
                })
                .invoke_handler(tauri::generate_handler![
                    super::super::create_project,
                    super::super::list_projects,
                    super::super::create_session,
                    super::super::update_session,
                    super::super::list_sessions,
                    super::super::delete_project,
                    super::super::move_session,
                    crate::pty::commands::write_pty,
                    crate::pty::commands::write_pty_bytes,
                    crate::pty::commands::resize_pty,
                    crate::pty::commands::ack_pty,
                    crate::session::stop_session,
                ])
                .build(mock_context(noop_assets()))
                .expect("build mock app")
        }

        /// JS の `invoke(cmd, args)` と同じ形（camelCase キーの JSON オブジェクト）で
        /// コマンドを叩き、成功時は返り値の JSON を返す。失敗時はテストを panic させる。
        fn invoke_ok(webview: &tauri::WebviewWindow<MockRuntime>, cmd: &str, body: Value) -> Value {
            let response = get_ipc_response(
                webview,
                InvokeRequest {
                    cmd: cmd.into(),
                    callback: CallbackFn(0),
                    error: CallbackFn(1),
                    url: "tauri://localhost".parse().expect("url"),
                    body: body.into(),
                    headers: Default::default(),
                    invoke_key: tauri::test::INVOKE_KEY.to_string(),
                },
            );
            match response {
                Ok(b) => b.deserialize::<Value>().expect("deserialize response"),
                Err(e) => panic!("{cmd} returned an error over IPC: {e}"),
            }
        }

        /// `invoke_ok` の失敗版。コマンドが `AppError` を返すことを期待するテスト用。
        /// 契約 §6 の `{"code": ..., "message": ...}` 形をそのまま返す。
        fn invoke_err(
            webview: &tauri::WebviewWindow<MockRuntime>,
            cmd: &str,
            body: Value,
        ) -> Value {
            let response = get_ipc_response(
                webview,
                InvokeRequest {
                    cmd: cmd.into(),
                    callback: CallbackFn(0),
                    error: CallbackFn(1),
                    url: "tauri://localhost".parse().expect("url"),
                    body: body.into(),
                    headers: Default::default(),
                    invoke_key: tauri::test::INVOKE_KEY.to_string(),
                },
            );
            match response {
                Ok(b) => panic!(
                    "{cmd} was expected to return an error over IPC but succeeded: {:?}",
                    b.deserialize::<Value>()
                ),
                Err(e) => e,
            }
        }

        // 変異 1: insert_project(&name, &repo_path, ...) の引数入れ替え検出。
        // name と repo_path を区別できる値にして、返ってきた Project の各フィールドが
        // 正しい方に入っていることを確認する。
        #[test]
        fn create_project_binds_name_and_repo_path_to_the_correct_fields() {
            let (_dir, store) = open_temp();
            let app = build_app(store);
            let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
                .build()
                .expect("build webview");

            let project = invoke_ok(
                &webview,
                "create_project",
                json!({
                    "name": "kamux",
                    "repoPath": "/Users/x/repo/kamux",
                    "defaultCli": "claude",
                }),
            );

            assert_eq!(project["name"], json!("kamux"));
            assert_eq!(project["repo_path"], json!("/Users/x/repo/kamux"));
        }

        // 変異 2: list_sessions が include_archived を無視して false 固定になっていないか。
        // 同じ project に非アーカイブ 1 件・アーカイブ 1 件を作り、includeArchived: true を
        // 実際に渡した場合だけ両方返ることを確認する（このレーンのどのテストも true を
        // 渡していなかった穴を塞ぐ）。
        #[test]
        fn list_sessions_honors_include_archived_true() {
            let (_dir, store) = open_temp();
            let app = build_app(store);
            let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
                .build()
                .expect("build webview");

            let project = invoke_ok(
                &webview,
                "create_project",
                json!({"name": "kamux", "repoPath": "/x/kamux", "defaultCli": "claude"}),
            );
            let project_id = project["id"].as_str().expect("project id").to_owned();

            let session_1 = invoke_ok(
                &webview,
                "create_session",
                json!({
                    "projectId": project_id,
                    "title": "archived one",
                    "description": "",
                    "mode": "in_place",
                    "branch": null,
                    "cliKind": "claude",
                    "cliCommand": null,
                }),
            );
            let session_1_id = session_1["id"].as_str().expect("session id").to_owned();

            invoke_ok(
                &webview,
                "create_session",
                json!({
                    "projectId": project_id,
                    "title": "active one",
                    "description": "",
                    "mode": "in_place",
                    "branch": null,
                    "cliKind": "claude",
                    "cliCommand": null,
                }),
            );

            invoke_ok(
                &webview,
                "update_session",
                json!({"id": session_1_id, "patch": {"archived_at": 1_700_000_000_000_i64}}),
            );

            let excluding_archived = invoke_ok(
                &webview,
                "list_sessions",
                json!({"projectId": project_id, "includeArchived": false}),
            );
            assert_eq!(
                excluding_archived.as_array().expect("array").len(),
                1,
                "includeArchived: false ではアーカイブ済みを除いた 1 件だけが返る"
            );

            let including_archived = invoke_ok(
                &webview,
                "list_sessions",
                json!({"projectId": project_id, "includeArchived": true}),
            );
            assert_eq!(
                including_archived.as_array().expect("array").len(),
                2,
                "includeArchived: true を実際に渡した場合はアーカイブ済みも含めて 2 件返る"
            );
        }

        // 変異 3: create_session が next_sort_order を使わず sort_order を定数にしていないか。
        // 同じ project に 2 件作って sort_order が採番どおり 1.0 -> 2.0 と増えることを見る。
        #[test]
        fn create_session_assigns_sort_order_from_next_sort_order() {
            let (_dir, store) = open_temp();
            let app = build_app(store);
            let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
                .build()
                .expect("build webview");

            let project = invoke_ok(
                &webview,
                "create_project",
                json!({"name": "kamux", "repoPath": "/x/kamux", "defaultCli": "claude"}),
            );
            let project_id = project["id"].as_str().expect("project id").to_owned();

            let session_1 = invoke_ok(
                &webview,
                "create_session",
                json!({
                    "projectId": project_id,
                    "title": "first",
                    "description": "",
                    "mode": "in_place",
                    "branch": null,
                    "cliKind": "claude",
                    "cliCommand": null,
                }),
            );
            let session_2 = invoke_ok(
                &webview,
                "create_session",
                json!({
                    "projectId": project_id,
                    "title": "second",
                    "description": "",
                    "mode": "in_place",
                    "branch": null,
                    "cliKind": "claude",
                    "cliCommand": null,
                }),
            );

            assert_eq!(session_1["sort_order"], json!(1.0));
            assert_eq!(session_2["sort_order"], json!(2.0));
        }

        // 変異 4: move_session の toStatus / toIndex が正しくバインドされているか。
        // 契約 §7.3 の落とし穴（camelCase 変換はコマンド引数名にしか効かない）は
        // move_session の引数がネスト構造を持たないため再現しないが、
        // 「toIndex を無視して末尾固定にしていないか」は実測しないと分からない。
        // review 列に既存 1 件を作った上で toIndex: 0 を渡し、末尾（sort_order 6.0
        // 相当）ではなく先頭（既存より小さい値）に入ることで toIndex の実効性を固定する。
        #[test]
        fn move_session_binds_camel_case_to_status_and_to_index() {
            let (_dir, store) = open_temp();
            let app = build_app(store);
            let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
                .build()
                .expect("build webview");

            let project = invoke_ok(
                &webview,
                "create_project",
                json!({"name": "kamux", "repoPath": "/x/kamux", "defaultCli": "claude"}),
            );
            let project_id = project["id"].as_str().expect("project id").to_owned();

            let existing = invoke_ok(
                &webview,
                "create_session",
                json!({
                    "projectId": project_id,
                    "title": "existing",
                    "description": "",
                    "mode": "in_place",
                    "branch": null,
                    "cliKind": "claude",
                    "cliCommand": null,
                }),
            );
            let existing_id = existing["id"].as_str().expect("session id").to_owned();
            invoke_ok(
                &webview,
                "update_session",
                json!({"id": existing_id, "patch": {"kanban_status": "review"}}),
            );

            let target = invoke_ok(
                &webview,
                "create_session",
                json!({
                    "projectId": project_id,
                    "title": "target",
                    "description": "",
                    "mode": "in_place",
                    "branch": null,
                    "cliKind": "claude",
                    "cliCommand": null,
                }),
            );
            let target_id = target["id"].as_str().expect("session id").to_owned();

            let column = invoke_ok(
                &webview,
                "move_session",
                json!({"id": target_id, "toStatus": "review", "toIndex": 0}),
            );

            let column = column.as_array().expect("array");
            assert_eq!(
                column.len(),
                2,
                "toStatus: review が review 列に反映されていない"
            );
            assert!(
                column
                    .iter()
                    .any(|s| s["id"] == json!(target_id) && s["sort_order"] == json!(0.0)),
                "toIndex: 0 が無視されて先頭挿入（既存より小さい値）になっていない"
            );
        }

        // --- Task 8 必達 2: PtyManager が AppState に配線されていることを検証する ---
        //
        // `state.rs` は Task 7 まで `pty` フィールドを持たずコメントのみだった。
        // 未登録の surface_id を渡すと `PtyManager::get_surface` が
        // `AppError::NotFound(surface_id)` を返す。これが IPC 越しにそのまま
        // 返ってくれば、camelCase キーのバインドと PtyManager への到達を
        // 同時に証明できる（`start_session` は `AppHandle` を要求するため
        // MockRuntime の invoke_handler に登録できず、この経路では検証できない。
        // 契約 392 行が `AppHandle`（Wry 固定）を明記しているのでシグネチャは
        // 変えない。report の CONCERNS に明記する）。
        #[test]
        fn write_pty_reaches_the_pty_manager_and_returns_not_found_for_an_unknown_surface() {
            let (_dir, store) = open_temp();
            let app = build_app(store);
            let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
                .build()
                .expect("build webview");

            let err = invoke_err(
                &webview,
                "write_pty",
                json!({"surfaceId": "nope:agent", "data": "hello"}),
            );

            assert_eq!(err["code"], json!("not_found"));
            assert_eq!(
                err["message"],
                json!("nope:agent"),
                "AppError::NotFound は surface_id をそのまま運ぶ(契約 §6)"
            );
        }

        // write_pty_bytes の base64 デコード分岐を検証する。デコードを飛ばして
        // 生バイトのまま `state.pty.write` に渡す変異を入れると、不正な base64
        // でも surface_id の解決まで進んでしまい `not_found` が返る(この
        // テストが期待する `io` と食い違って赤くなる)。
        #[test]
        fn write_pty_bytes_rejects_invalid_base64_before_reaching_the_pty_manager() {
            let (_dir, store) = open_temp();
            let app = build_app(store);
            let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
                .build()
                .expect("build webview");

            let err = invoke_err(
                &webview,
                "write_pty_bytes",
                json!({"surfaceId": "nope:agent", "base64": "!!!not-base64!!!"}),
            );

            assert_eq!(err["code"], json!("io"));
            assert!(
                err["message"]
                    .as_str()
                    .expect("message")
                    .contains("invalid base64 payload"),
                "actual message: {:?}",
                err["message"]
            );
        }

        // stop_session が `SurfaceKind::Agent` の surface_id を kill することを検証する。
        // `session::stop_session` の内部を `Editor` に変異させると kill 対象が
        // ずれて `s:agent` が生き残ったままになり、この assert が赤くなる。
        #[test]
        fn stop_session_kills_the_agent_surface_registered_for_the_session() {
            use std::sync::mpsc::channel;
            use std::time::Duration;

            use crate::model::SurfaceKind;
            use crate::pty::surface::PtySink;
            use crate::pty::{surface_id, SpawnSpec, DEFAULT_COLS, DEFAULT_ROWS};

            struct ExitSink(std::sync::mpsc::Sender<()>);
            impl PtySink for ExitSink {
                fn on_data(&self, _surface_id: &str, _base64: String, _seq: u64) {}
                fn on_exit(&self, _surface_id: &str, _exit_code: Option<i32>) {
                    let _ = self.0.send(());
                }
            }

            let (_dir, store) = open_temp();
            let app = build_app(store);
            let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
                .build()
                .expect("build webview");

            let project = invoke_ok(
                &webview,
                "create_project",
                json!({"name": "kamux", "repoPath": "/x/kamux", "defaultCli": "claude"}),
            );
            let project_id = project["id"].as_str().expect("project id").to_owned();
            let session = invoke_ok(
                &webview,
                "create_session",
                json!({
                    "projectId": project_id,
                    "title": "shell session",
                    "description": "",
                    "mode": "in_place",
                    "branch": null,
                    "cliKind": "shell",
                    "cliCommand": null,
                }),
            );
            let session_id = session["id"].as_str().expect("session id").to_owned();
            let agent_surface_id = surface_id(&session_id, SurfaceKind::Agent);

            let (tx, rx) = channel();
            let state = app.state::<AppState>();
            state
                .pty
                .spawn_with_sink(
                    std::sync::Arc::new(ExitSink(tx)),
                    SpawnSpec {
                        surface_id: agent_surface_id.clone(),
                        program: "/bin/cat".to_string(),
                        args: Vec::new(),
                        cwd: std::path::PathBuf::from("/tmp"),
                        env: Vec::new(),
                        cols: DEFAULT_COLS,
                        rows: DEFAULT_ROWS,
                    },
                )
                .expect("spawn the agent surface directly for the test");
            assert!(state.pty.is_alive(&agent_surface_id));

            invoke_ok(&webview, "stop_session", json!({"id": session_id}));

            rx.recv_timeout(Duration::from_secs(10))
                .expect("agent surface must exit within 10s of stop_session");
            assert!(
                !state.pty.is_alive(&agent_surface_id),
                "stop_session must kill the SurfaceKind::Agent surface for the session"
            );
        }
    }
}
