// モジュールはすべて pub mod で宣言する。private mod にすると dead_code が消えない（§45.1 の実測）
pub mod error;
pub mod model;
pub mod notify;
pub mod pty;
pub mod session;
pub mod state;
pub mod store;
pub mod worktree;

use std::sync::Arc;

use tauri::{Manager, State, WindowEvent};

use crate::error::AppResult;
use crate::model::{CliKind, KanbanStatus, Project, Session, SessionMode, SessionPatch};
use crate::session::runtime_state::{RuntimeStateManager, StatePersist, TauriEmitObserver};
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

/// 終了時の後始末のうち、**順序を型で強制する 2 手**だけをここに閉じる。
///
/// 効いているのはモジュールの private ではなく **`ShutdownBegun` のフィールドの
/// private** である。タプルフィールドが private なので、コンストラクタもこの
/// モジュールの外からは見えない（E0603）。したがって `kill_all_pty_surfaces` を
/// 呼ぶ唯一の方法は `begin_runtime_shutdown` の戻り値を渡すことになる。
///
/// **`shutdown_runtime_then_kill_pty` をこのモジュールへ移してはいけない。**
/// 中に入れるとフィールドが可視になり `kill_all_pty_surfaces(ShutdownBegun(()), ..)`
/// と書けてしまい、逆順がコンパイルを通る（= 保証が消える）。
/// 同じ理由で **`ShutdownBegun` をフィールドの無いユニット構造体に戻してはいけない。**
mod shutdown {
    use crate::pty::PtyManager;
    use crate::session::runtime_state::RuntimeStateManager;

    /// 「`RuntimeStateManager::begin_shutdown()` を既に呼んだ」ことの証拠。
    /// `kill_all_pty_surfaces` はこれを引数に要求する。
    ///
    /// **飾りではない。** モジュール外ではこの型を構築できないので、
    /// `shutdown_runtime_then_kill_pty` の 2 行を逆順に書くと `cargo build` が
    /// 落ちる（トークンを作れば E0603 / 束縛前に使えば E0425）。
    ///
    /// この形にしているのは、**現時点では**順序を実行時に観測する手段が無いため
    /// である —— `begin_shutdown()` の後に届いた入力は `consume_loop` の先頭ゲートで
    /// 捨てられ、メモリ・DB・observer のどこにも痕跡を残さない。一方 PTY の kill から
    /// `PtyExited` が観測されるまでの遅延（子プロセスの死を waiter が拾うまでの数 ms）は、
    /// 逆順に書いても実測ではまず表面化しない。つまり今は「順序が逆でも緑になるテスト」
    /// しか書けない。
    ///
    /// **これは無期限の事実ではない。** 観測できないのは PTY kill -> 状態機械の入力
    /// （`pty/sink.rs` の `note_pty_exit`）が **Task 7 でまだ配線されていない**からである。
    /// Task 7 が入れば「実 PTY を spawn -> teardown -> DB に `exited` が焼かれていない
    /// こと」で順序を実行時にも固定できる（Task 6 レビュー §6.3 の持ち越し）。
    /// そのときもこの型を外さないこと —— 型と実行時テストは競合しない。
    ///
    /// 順序を間違えたときに実機で起きること: Cmd+Q の一括 kill 由来の `PtyExited` が
    /// 状態機械に届き、DB に `exited` が焼かれる。次回起動で `normalize_on_startup` は
    /// `{running, waiting_input}` しか触らないので、全カードが ⏸ ではなく ⛔ で復帰する
    /// （計画 §4.4 / §6.2）。
    pub struct ShutdownBegun(());

    pub fn begin_runtime_shutdown(runtime: &RuntimeStateManager) -> ShutdownBegun {
        runtime.begin_shutdown();
        ShutdownBegun(())
    }

    /// 生存中の全 PTY サーフェスを一括 kill する。`PtyManager::kill` は契約 §15 で
    /// 冪等と規定されているため、この関数自体も何度呼んでも安全（Task 12.5 番外・
    /// RULINGS §23.1: WindowEvent::Destroyed と RunEvent::Exit の両方から同じ kill を
    /// 撃つ設計を選んだのはこの冪等性が前提）。
    ///
    /// この関数自体（「生きている surface_id を集めて機械的に kill する」という
    /// ロジック）は `tests::kill_all_pty_surfaces_kills_every_live_surface` が
    /// 実プロセスの spawn/kill/exit を通しで固定している。トークンを偽造できないので、
    /// そのテストは唯一の入口である `shutdown_runtime_then_kill_pty` 経由で呼ぶ。
    pub fn kill_all_pty_surfaces(_begun: ShutdownBegun, pty: &PtyManager) {
        for id in pty.live_surfaces() {
            let _ = pty.kill(&id);
        }
    }
}

/// アプリ終了時の後始末。**順序を持つ 2 手をこの 1 箇所に閉じ込める。**
///
/// **この関数は `mod shutdown` の外に置くこと。** 中へ移すと `ShutdownBegun` の
/// フィールドが可視になり、逆順に書いてもコンパイルが通るようになる（`mod shutdown`
/// の doc コメント参照）。
///
/// 呼び出し元は `kill_on_window_destroyed`（窓を閉じる経路）と
/// `kill_on_run_event_exit`（Cmd+Q の経路）の 2 つ。両方から撃つのは
/// RULINGS §23.1 が kill について採った設計をそのまま踏襲したもので、
/// `begin_shutdown()`（`AtomicBool`）も `kill_all_pty_surfaces` も冪等なので
/// 二重呼び出しは無害である。
///
/// **`RunEvent::ExitRequested` には置かない。** Cmd+Q は `terminate:` ->
/// `applicationWillTerminate:` -> `AppState::exit()` -> `LoopDestroyed` ->
/// `RunEvent::Exit` と進み、`ExitRequested` を経由しない（`kill_on_run_event_exit`
/// の doc コメントが依存クレートのソースを追跡して確定させた経路）。
fn shutdown_runtime_then_kill_pty(state: &AppState) {
    let begun = shutdown::begin_runtime_shutdown(&state.runtime);
    shutdown::kill_all_pty_surfaces(begun, &state.pty);
}

/// 窓が閉じる経路（赤ボタンなど）で PTY の子プロセスを残さない。
///
/// Cmd+Q はここを通らない。PR 単位レビューが依存クレートのソースを追跡して
/// 確定した（tauri 2.11.5 / tao 0.35.3 / muda 0.19.3。実機起動での観測は
/// Task 16 の手動スモークが行う）。既定の macOS メニューの
/// `PredefinedMenuItem::quit` は `sel!(terminate:)` を送り、
/// `[NSApp terminate:]` は個々の窓を閉じずに直接 `applicationWillTerminate:`
/// へ進む。`WindowEvent::Destroyed` を発火させる tao の経路は
/// `windowWillClose:` の 1 箇所のみで、`terminate:` はこれを経由しない。
/// そのため Cmd+Q の後始末は `kill_on_run_event_exit` が担う。
///
/// それでもここを消さずに残すのは、macOS で最後の窓を閉じてもプロセスが
/// 終了しない経路があるため（RULINGS §23.1）。その場合 `RunEvent::Exit` は
/// 飛ばないが、この Destroyed 側でサーフェスは消える。`kill_all_pty_surfaces`
/// は冪等なので両方から撃っても安全（ledger の `Task 6: minor (deferred)`
/// が実測で記録済み）。
///
/// `R: tauri::Runtime` はテストのために足した内部実装の一般化であり、契約上の
/// 型ではない（`TauriSink<R: Runtime = Wry>` と同じ前例。`pty/sink.rs` 参照）。
/// production の `run()` は `tauri::Builder::default()`（= Wry）のまま呼ぶため
/// 挙動は変わらない。
///
/// `MockRuntime` は `WindowEvent::Destroyed` を一度も発火しない
/// （`tauri-2.11.5/src/test/mock_runtime.rs` を `grep -n "Destroyed"` した
/// 結果が 0 件であることを確認済み）ため、「Destroyed イベントの配送」を
/// 経由した経路は守るテストが無い。ただし `WindowEvent::Destroyed` は
/// フィールドの無いユニットバリアントであり、enum 全体に付いた
/// `#[non_exhaustive]` があってもクレート外から構築できる
/// （`let _ = tauri::WindowEvent::Destroyed;` がこのクレートからコンパイルを
/// 通ることを確認済み）ため、この関数自体は直接呼んで固定できる。この形で
/// 固定しているテストは 2 本ある:
/// - `tests::kill_on_window_destroyed_kills_every_live_pty_surface_when_called_directly`
///   （PTY の一括 kill。M1-4）
/// - `tests::wiring::window_destroyed_begins_runtime_shutdown`
///   （状態機械の shutdown。M2-1 Task 6）
///
/// **この制約は E2E でも解消しない。** `MockRuntime` に発火経路が無いのは
/// テストの書き方の問題ではなくランタイムの実装の問題なので、「E2E なら
/// Destroyed の配送まで通せるはず」と考えて書き直さないこと。実配送の確認は
/// 実機の手動スモーク（Task 16）でしか取れない。
fn kill_on_window_destroyed<R: tauri::Runtime>(window: &tauri::Window<R>, event: &WindowEvent) {
    if matches!(event, WindowEvent::Destroyed) {
        if let Some(state) = window.try_state::<AppState>() {
            shutdown_runtime_then_kill_pty(&state);
        }
    }
}

/// `RunEvent::Exit` でも `kill_on_window_destroyed` と同じ一括 kill を撃つ。
///
/// Cmd+Q はこの経路を通る。PR 単位レビューが依存クレートのソースを追跡して
/// 確定した（tauri 2.11.5 / tao 0.35.3 / muda 0.19.3 / tauri-runtime-wry
/// 2.11.4。実機起動での観測は Task 16 の手動スモークが行う）: 既定メニューの
/// `PredefinedMenuItem::quit` が送る `terminate:` を tao の
/// `applicationWillTerminate:` が受け、`AppState::exit()` ->
/// `Event::LoopDestroyed` を経て tauri-runtime-wry がこれを `RunEvent::Exit`
/// に変換する。`kill_on_window_destroyed` と両方から撃つ理由はそちらの doc
/// コメント参照。
///
/// `R: tauri::Runtime` はテストのために足した内部実装の一般化（上記と同じ
/// 前例）。`MockRuntime` は最後の窓が消えたあと必ず `RunEvent::Exit` を
/// 発火する（`tauri-2.11.5/src/test/mock_runtime.rs:1393`）ため、この関数は
/// `tests::run_event_exit_kills_every_live_pty_surface` から実際に到達できる。
fn kill_on_run_event_exit<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    event: tauri::RunEvent,
) {
    if matches!(event, tauri::RunEvent::Exit) {
        if let Some(state) = app_handle.try_state::<AppState>() {
            shutdown_runtime_then_kill_pty(&state);
        }
    }
}

/// `setup` クロージャの本体。
///
/// `&mut App<R>` ではなく `&M: Manager<R>` を受けるのは、`tauri::test::mock_builder()`
/// で組んだ `App<MockRuntime>` から**同じ経路**をテストするため（`R: Runtime` を足す
/// 理由は `TauriSink` / `kill_on_run_event_exit` と同じで、契約上の型ではない）。
///
/// **`AppState` の構築を `Builder::manage(..)` ではなく `setup` の中で行う理由:**
/// `TauriEmitObserver` に `AppHandle` を渡す必要があり、`AppHandle` は `setup` の
/// 中でしか取れない。コマンドは `setup` 完了後にしか実行されないので、この置き場所で
/// `State<'_, AppState>` が未登録になる瞬間は生じない。
fn install_app_state<R: tauri::Runtime, M: Manager<R>>(manager: &M, store: Arc<Store>) {
    let persist = Arc::clone(&store) as Arc<dyn StatePersist>;
    install_app_state_with(manager, store, persist);
}

/// `install_app_state` の注入版（`session::plan_agent_spawn_with` と同じ seam）。
///
/// `persist` を引数に出しているのは、起動時正規化の**呼ばれ方**（回数・順序・失敗時の
/// 扱い）をフェイクで観測するため。実 `Store` を差す結線そのものは `install_app_state`
/// 側にあり、そちらは DB を読み直すテストで固定する。
fn install_app_state_with<R: tauri::Runtime, M: Manager<R>>(
    manager: &M,
    store: Arc<Store>,
    persist: Arc<dyn StatePersist>,
) {
    let runtime = RuntimeStateManager::new(persist);
    runtime.register_observer(Arc::new(TauriEmitObserver::new(
        manager.app_handle().clone(),
    )));

    // 前回 running / waiting_input のまま終了した行を interrupted へ正規化する。
    //
    // **`manage` より前に完了させること。** `AppState` が manage された瞬間から、
    // コマンドと `sink.rs` が `state.runtime.sender()` へ到達して入力を積めるようになる。
    // `normalize_on_startup` は読み取りフェーズを write lock の外で回してから
    // `map.insert` を無条件に行うので、その隙間に consumer が進めた状態が
    // DB 由来の古い値で巻き戻る（PR 13 レビュー I-2）。「順序を守る」ではなく
    // 「まだ配らない」で塞ぐ —— `sender()` は `&self` なので、返した時点で誰でも
    // 入力を積めてしまう。
    //
    // WebView はまだ listen していないので、ここではイベントを出さない（計画 §4.9）。
    // フロントは `list_sessions` の戻り値（正規化済みの `last_runtime_state`）から
    // シードする。
    match runtime.normalize_on_startup() {
        Ok(changed) if !changed.is_empty() => {
            eprintln!(
                "[kamux] normalized {} interrupted session(s)",
                changed.len()
            );
        }
        Ok(_) => {}
        // 走査途中で失敗しても**ログに落として起動を続行する**（Task 6 必達 5 の決定）。
        // 理由は 3 つ:
        //   1. 契約 §0 が panic 経路を禁じている。DB の 1 行が書けないだけでアプリが
        //      起動しないのは過剰な対応である
        //   2. `consume_loop` が「DB 書き込み失敗で状態機械を止めない」としているのと
        //      同じ規律。真実はメモリ側にある
        //   3. 正規化は冪等なので、途中まで書けた状態は次回起動で自己修復する
        //      （昇格済みの行は `interrupted` として据え置かれ、残った `running` の行が
        //      改めて昇格する）
        //
        // **代償（正確に書く。「⏸ にならないだけ」ではない）:** `normalize_on_startup` は
        // `set_last_runtime_state` に `?` を使うため、**失敗した行より後ろは `map.insert`
        // されない**。つまり DB を昇格できないだけでなく、**未走査の行がメモリスナップ
        // ショットにも載らない** —— たとえば `last_runtime_state='error'` の行に対して
        // `current(id)` が `Idle` を返す。同関数の doc コメントが警告している split-brain
        // （❌ のカードが `HookNotification` 1 発で 🟡 へ復帰する）が、**その起動の間ずっと
        // 残る**。次回起動の正規化で自己修復する。起動失敗より軽いという判断は変わらない。
        //
        // 走査を続行して最後にまとめてエラーを返す形（部分シードを無くす）は
        // `normalize_on_startup` 本体 = PR 13 の改変になるため、Task 6 では入れず
        // lane-controller への持ち越しとする（Task 6 レビュー I-2）。
        Err(err) => eprintln!("[kamux] startup normalize failed: {err}"),
    }

    manager.manage(AppState {
        store,
        pty: pty::PtyManager::new(),
        runtime,
    });
}

// 契約 §45.2: tauri::Builder の組み立てとコマンド登録は lib.rs の run() の中だけに置く。
// main.rs は `fn main() { kamux::run() }` の 3 行で固定であり、以後どの計画も編集しない。
pub fn run() {
    let app = tauri::Builder::default()
        .setup(|app| {
            // 契約 §17: db_path() は環境変数 KAMUX_DB_PATH で上書き可
            let store = Arc::new(Store::open(&db_path()?)?);
            install_app_state(app, store);

            // Task 7 スパイク（M2-3 PR 19）: KAMUX_NOTIFY_SPIKE=1 のときだけ動く。
            // Task 10 で本番の配線に置き換えるため、Task 10 のコミットで削除する。
            if std::env::var("KAMUX_NOTIFY_SPIKE").is_err() {
                return Ok(());
            }

            // 要検証 1: バンドル ID があるか
            match notify_rust::check_bundle() {
                Ok(()) => println!("[OK] check_bundle"),
                Err(e) => println!("[NG] check_bundle failed: {e}"),
            }

            // 要検証 4-a: 起動経路で権限読み取りがハングしないか（起動 1 秒未満の制約）
            let t0 = std::time::Instant::now();
            match notify_rust::get_notification_settings_blocking() {
                Ok(s) => println!(
                    "[--] authorization_status = {:?} ({} ms)",
                    s.authorization_status,
                    t0.elapsed().as_millis()
                ),
                Err(e) => println!("[NG] get_notification_settings: {e}"),
            }

            // Task 7 修正ラウンド 1（M2-3 PR 19）: 以下 2 行は測定であって
            // 守りではない。Task 10 の本番配線ではこの if ブロックごと消す。
            //
            // 測定 1: setup はまだイベントループ開始前なので false のはず
            // （違えば「ループ未開始でも true になる」という重要な発見なので
            // そのまま報告する）。
            //
            // `CFRunLoop::main().is_some_and(|rl| rl.is_waiting())` は
            // `mac-usernotifications` **0.3.1** の `main_run_loop_is_running()`
            // （`pub(crate)` でクレート外から呼べないため再構成した式。
            // `src/lib.rs:232-234`）の再構成である。**0.3.1 の式であり、
            // 他の版は測っていない。** 上流が式を変えたらこの測定は古い述語を
            // 測ったことになる（詳細は `run_notify_spike_after_ready` の
            // doc コメント参照）。
            let waiting_at_setup =
                objc2_core_foundation::CFRunLoop::main().is_some_and(|rl| rl.is_waiting());
            println!("[MEASURE-1] setup: main_run_loop_is_waiting = {waiting_at_setup}");

            // 測定 3-a: setup はメインスレッドで走るので true のはず。
            let marker_at_setup = objc2::MainThreadMarker::new().is_some();
            println!(
                "[MEASURE-3a] setup (main thread): main_thread_marker_is_some = {marker_at_setup}"
            );

            Ok(())
        })
        .on_window_event(kill_on_window_destroyed)
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
            session::suggest_branch_name,
            crate::pty::editor::spawn_editor,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build kamux");

    app.run(move |handle, event| {
        // Task 7 修正ラウンド 1（M2-3 PR 19）: `RunEvent::Ready` を合図に
        // 測定 2・3-b と試行 A・B を行う。`kill_on_run_event_exit` は
        // これまでどおり全イベントで呼び続ける（2 本のテストがこれに依存する）。
        if matches!(event, tauri::RunEvent::Ready) && std::env::var("KAMUX_NOTIFY_SPIKE").is_ok() {
            let spike_handle = handle.clone();
            std::thread::spawn(move || run_notify_spike_after_ready(spike_handle));
        }
        kill_on_run_event_exit(handle, event);
    });
}

/// Task 7 修正ラウンド 1（M2-3 PR 19）: `RunEvent::Ready` 直後にバックグラウンド
/// スレッドから呼ばれる。測定 2・3-b と試行 A・B（`mac-usernotifications` の
/// `block_on_current` がどちらの枝を通るかの実地確認）を行う。
///
/// **測定 2 はこの関数の呼び出し元（バックグラウンドスレッド）から行うこと。**
/// `Mainthread not running` を出している当の述語
/// (`mac-usernotifications-0.3.1::block_on_current` -> `main_run_loop_is_running`)
/// がバックグラウンドスレッドから `CFRunLoop::main().is_waiting()` を評価して
/// いるため、メインスレッドから測ると測定コード自身の実行によって
/// `is_waiting` がほぼ常に false になり、別の量を測ることになる。
///
/// **測定 2・3-b が使う `CFRunLoop::main().is_some_and(|rl| rl.is_waiting())` は、
/// `mac-usernotifications` **0.3.1** の `main_run_loop_is_running()`
/// （`pub(crate)` でクレート外から呼べないため再構成した。
/// `src/lib.rs:232-234`）の再構成であって、述語そのものではない。**
/// **0.3.1 の式であり、他の版は測っていない。** 上流が式を変えたら、
/// この測定は古い述語を測ったことになる。
///
/// 測定コード（`[MEASURE-*]` を出す部分）は測定であって守りではない。
/// Task 10 の本番配線ではこの関数ごと消す。
fn run_notify_spike_after_ready(handle: tauri::AppHandle<tauri::Wry>) {
    // 測定 3-b: バックグラウンドスレッドなので false のはず。
    let marker_on_thread = objc2::MainThreadMarker::new().is_some();
    println!("[MEASURE-3b] spawned thread:      main_thread_marker_is_some = {marker_on_thread}");

    // 測定 2: Ready 直後から 100 回、2ms 間隔で is_waiting をサンプリングする。
    // 間隔の sleep は測定のためであり許される（下の試行 A/B には壁時計 sleep
    // を入れない）。生の列も出す —— 比率だけだと分布（振動しているか、
    // 一貫して false か）が消える。
    const SAMPLES: usize = 100;
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let waiting = objc2_core_foundation::CFRunLoop::main().is_some_and(|rl| rl.is_waiting());
        samples.push(waiting);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let true_count = samples.iter().filter(|&&b| b).count();
    let false_count = SAMPLES - true_count;
    let ratio = true_count as f64 / SAMPLES as f64;
    println!(
        "[MEASURE-2] samples={SAMPLES} interval=2ms true={true_count} false={false_count} ratio={ratio:.2}"
    );
    let first20 = samples
        .iter()
        .take(20)
        .map(|b| if *b { "t" } else { "f" })
        .collect::<Vec<&str>>()
        .join(",");
    println!("[MEASURE-2] first20={first20}");

    let granted = notify_rust::request_auth_blocking().unwrap_or(false);
    println!("[--] request_auth -> {granted}");

    // 試行 A: そのまま show()。
    let want_id_a = "kamux-session-spike-a".to_string();
    let mut n_a = notify_rust::Notification::new();
    n_a.summary("A: 入力待ち")
        .body("kamux · session/spike (A: そのまま show)")
        .id(notify_rust::NotificationId::Mac(want_id_a.clone()))
        .timeout(notify_rust::Timeout::Milliseconds(30_000));
    let a_handle = match n_a.show() {
        Ok(h) => {
            println!("[TRY-A] show -> ok id={want_id_a}");
            match h.id() {
                notify_rust::NotificationId::Mac(ref got) if *got == want_id_a => {
                    println!("[OK] notification id round-trips: {got}")
                }
                other => println!("[NG] notification id was rewritten: {other:?}"),
            }
            Some(h)
        }
        Err(e) => {
            println!("[TRY-A] show -> failed: {e}");
            None
        }
    };

    // 試行 B: run_on_main_thread 経由で show()。
    // A の成否に関わらず必ず試す —— ゲートは 1 回しか回らない。
    // B の中では wait_for_response を呼ばない（メインスレッドを 30 秒
    // ブロックして UI が固まる。見たいのは show() が通るかだけなので、
    // 返ったハンドルはそのまま drop する）。
    //
    // A の `wait_for_response`（最大 30 秒ブロック）より先に済ませることで、
    // B の通知が A の陰で 30 秒以上遅れて出るのを避ける。
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let run_result = handle.run_on_main_thread(move || {
        let marker_inside = objc2::MainThreadMarker::new().is_some();
        let want_id_b = "kamux-session-spike-b".to_string();
        let mut n_b = notify_rust::Notification::new();
        n_b.summary("B: 入力待ち")
            .body("kamux · session/spike (B: run_on_main_thread)")
            .id(notify_rust::NotificationId::Mac(want_id_b.clone()))
            .timeout(notify_rust::Timeout::Milliseconds(30_000));
        // ハンドルは drop する（上のコメントのとおり）。
        let show_result = match n_b.show() {
            Ok(_h) => format!("[TRY-B] show -> ok id={want_id_b}"),
            Err(e) => format!("[TRY-B] show -> failed: {e}"),
        };
        let _ = tx.send(format!(
            "[TRY-B] main_thread_marker_is_some_inside = {marker_inside}\n{show_result}"
        ));
    });
    match run_result {
        Ok(()) => match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(msg) => println!("{msg}"),
            Err(e) => println!("[NG] TRY-B result channel: {e}"),
        },
        Err(e) => println!("[NG] run_on_main_thread failed: {e}"),
    }

    // 試行 A が成功していれば、これまでどおり id 往復に続けて
    // wait_for_response / Dock バッジの観測を続ける。B の後に置くことで
    // B の通知が A の 30 秒ブロックに巻き込まれない。
    if let Some(h) = a_handle {
        let r = h.wait_for_response(|resp: &notify_rust::NotificationResponse| {
            println!(
                "[--] response = {resp:?} (is_default_action = {})",
                resp.is_default_action()
            );
        });
        println!("[--] wait_for_response returned: {r:?}");

        // 要検証 4-b: Dock バッジ
        use tauri::Manager;
        if let Some(w) = handle.get_webview_window("main") {
            let _ = w.set_badge_count(Some(3));
            std::thread::sleep(std::time::Duration::from_secs(3));
            let _ = w.set_badge_count(None);
            println!("[--] badge: 3 を 3 秒表示したあと None で消した");
        }
    }
    println!("[OK] spike finished without hanging");
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
        let state = crate::state::test_support::app_state(store);

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

    // --- Task 12.5（番外）: RunEvent::Exit / WindowEvent::Destroyed の両方から
    // 撃つ一括 kill ロジックを純関数へ切り出し、そこだけを実測で固定する。
    #[test]
    fn kill_all_pty_surfaces_kills_every_live_surface() {
        use std::sync::mpsc::channel;
        use std::time::Duration;

        use crate::pty::surface::PtySink;
        use crate::pty::{SpawnSpec, DEFAULT_COLS, DEFAULT_ROWS};

        struct ExitSink(std::sync::mpsc::Sender<String>);
        impl PtySink for ExitSink {
            fn on_data(&self, _surface_id: &str, _base64: String, _seq: u64) {}
            fn on_exit(&self, surface_id: &str, _exit_code: Option<i32>) {
                let _ = self.0.send(surface_id.to_string());
            }
        }

        // `ShutdownBegun` はフィールドが private なので `mod shutdown` の外からは
        // 構築できない（それが逆順をコンパイルエラーにしている当のもの）。テスト用の
        // 裏口を開けるのではなく、唯一の入口である `shutdown_runtime_then_kill_pty`
        // 経由で kill ループへ到達する。`begin_shutdown()` は状態機械の入力ゲートを
        // 閉じるだけで PTY の kill / `on_exit` の配送には触らないので、このテストが
        // 見ているもの（生きている surface が全部死ぬこと）は変わらない。
        let (_dir, store) = open_temp();
        let state = crate::state::test_support::app_state(store);
        let pty = &state.pty;
        let (tx, rx) = channel();
        let sink: Arc<dyn PtySink> = Arc::new(ExitSink(tx));
        for i in 0..2 {
            pty.spawn_with_sink(
                Arc::clone(&sink),
                SpawnSpec {
                    surface_id: format!("kill-all-{i}:agent"),
                    program: "/bin/cat".to_string(),
                    args: Vec::new(),
                    cwd: std::path::PathBuf::from("/tmp"),
                    env: Vec::new(),
                    cols: DEFAULT_COLS,
                    rows: DEFAULT_ROWS,
                },
            )
            .expect("spawn");
        }
        assert_eq!(
            pty.live_surfaces().len(),
            2,
            "both surfaces must be live before kill"
        );

        // このテストは「生きている surface を機械的に kill する」ループを見る。
        // begin_shutdown との順序は `ShutdownBegun` のフィールド private が
        // コンパイル時に固定しているので、ここでは実行時に確かめない。
        super::shutdown_runtime_then_kill_pty(&state);

        for _ in 0..2 {
            rx.recv_timeout(Duration::from_secs(10))
                .expect("each surface must exit after kill_all_pty_surfaces");
        }
        assert!(
            pty.live_surfaces().is_empty(),
            "kill_all_pty_surfaces must kill every surface it was told was live"
        );
    }

    // 「いつ」kill_all_pty_surfaces を呼ぶかの配線のうち、`RunEvent::Exit` 側は
    // MockRuntime でも到達できる。`MockRuntime::run` は最後の窓が消えたあと
    // 必ず `RunEvent::Exit` を発火する（tauri-2.11.5/src/test/mock_runtime.rs
    // の `run` 実装の末尾、ループを抜けた直後の `callback(RunEvent::Exit)`）。
    // 一方 `WindowEvent::Destroyed` 側は MockRuntime に発火経路が無いため
    // （同ファイルを grep しても Destroyed を作る箇所は 0 件）、こちらは
    // 引き続き「守っているテストは無い」。
    #[test]
    fn run_event_exit_kills_every_live_pty_surface() {
        use std::sync::mpsc::channel;
        use std::time::Duration;

        use tauri::test::{mock_builder, mock_context, noop_assets};
        use tauri::{Manager, WebviewWindowBuilder};

        use crate::pty::surface::PtySink;
        use crate::pty::{SpawnSpec, DEFAULT_COLS, DEFAULT_ROWS};
        use crate::store::test_support::open_temp;

        struct ExitSink(std::sync::mpsc::Sender<String>);
        impl PtySink for ExitSink {
            fn on_data(&self, _surface_id: &str, _base64: String, _seq: u64) {}
            fn on_exit(&self, surface_id: &str, _exit_code: Option<i32>) {
                let _ = self.0.send(surface_id.to_string());
            }
        }

        let (_dir, store) = open_temp();
        let app = mock_builder()
            .manage(crate::state::test_support::app_state(store))
            .build(mock_context(noop_assets()))
            .expect("build mock app");

        // 窓が 1 つも無いと MockRuntime::run はいつまでも「窓が空になった」を
        // 検知できず Exit まで進まないため、close する対象として 1 枚作る。
        let window = WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("build webview");

        let app_handle = app.handle().clone();
        let (exit_tx, exit_rx) = channel();
        let sink: Arc<dyn PtySink> = Arc::new(ExitSink(exit_tx));
        {
            let state = app_handle.state::<AppState>();
            state
                .pty
                .spawn_with_sink(
                    Arc::clone(&sink),
                    SpawnSpec {
                        surface_id: "run-event-exit:agent".to_string(),
                        program: "/bin/cat".to_string(),
                        args: Vec::new(),
                        cwd: std::path::PathBuf::from("/tmp"),
                        env: Vec::new(),
                        cols: DEFAULT_COLS,
                        rows: DEFAULT_ROWS,
                    },
                )
                .expect("spawn");
            assert!(state.pty.is_alive("run-event-exit:agent"));
        }

        // MockRuntime::run はメッセージが無ければ 1 秒スリープを挟んで
        // ポーリングする。`window.close()` を送るのが早すぎると
        // `is_running` が立つ前に処理されてしまい、待っている相手が
        // 誰も居ないまま `CloseWindow` が握りつぶされる（`RuntimeContext::
        // send_message` は `is_running` が false ならチャンネルを経由せず
        // 即座に窓を消すだけで、`RunEvent::Exit` を発火する経路を通らない）。
        // そこで `RunEvent::Ready`（ループ開始直後に必ず飛ぶ）を合図に
        // `close()` を送ることで、ポーリング待ちのレースを避ける。
        let (ready_tx, ready_rx) = channel::<()>();
        let run_thread = std::thread::spawn(move || {
            app.run(move |handle, event| {
                if matches!(event, tauri::RunEvent::Ready) {
                    let _ = ready_tx.send(());
                }
                super::kill_on_run_event_exit(handle, event);
            });
        });

        ready_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("event loop must reach RunEvent::Ready before we close the window");
        window
            .close()
            .expect("close the only window to drive the loop to RunEvent::Exit");

        run_thread.join().expect("event loop thread must not panic");

        exit_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("RunEvent::Exit must have killed the pty surface");
        assert!(
            !app_handle.state::<AppState>().pty.is_alive("run-event-exit:agent"),
            "kill_on_run_event_exit must kill every surface that was live when RunEvent::Exit fired"
        );
    }

    // 「Destroyed が来たときに kill する」経路のうち、MockRuntime がイベント配送
    // 自体を再現できない部分（実測: 上の run_event_exit テストの `Destroyed`
    // grep が 0 件）を除いた「関数本体がロジックとして正しいか」は、
    // `tauri::WindowEvent::Destroyed` がフィールドの無いユニットバリアントで
    // あり `#[non_exhaustive]` が付いていてもクレート外から構築できる
    // （実測: `let _ = tauri::WindowEvent::Destroyed;` がこのクレートから
    // コンパイルを通ることを確認済み）ため、関数を直接呼ぶ形で固定できる。
    #[test]
    fn kill_on_window_destroyed_kills_every_live_pty_surface_when_called_directly() {
        use std::sync::mpsc::channel;
        use std::time::Duration;

        use tauri::test::{mock_builder, mock_context, noop_assets};
        use tauri::{Manager, WebviewWindowBuilder, WindowEvent};

        use crate::pty::surface::PtySink;
        use crate::pty::{SpawnSpec, DEFAULT_COLS, DEFAULT_ROWS};
        use crate::store::test_support::open_temp;

        struct ExitSink(std::sync::mpsc::Sender<String>);
        impl PtySink for ExitSink {
            fn on_data(&self, _surface_id: &str, _base64: String, _seq: u64) {}
            fn on_exit(&self, surface_id: &str, _exit_code: Option<i32>) {
                let _ = self.0.send(surface_id.to_string());
            }
        }

        let (_dir, store) = open_temp();
        let app = mock_builder()
            .manage(crate::state::test_support::app_state(store))
            .build(mock_context(noop_assets()))
            .expect("build mock app");
        WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("build webview");
        // `on_window_event` は `&tauri::Window<R>` を渡す。`WebviewWindow` は
        // それを直接公開しないため（`WindowBuilder` は unstable フィーチャ
        // 越しでしか使えない）、`Manager::get_window` で同じラベルの
        // `Window<R>` を引き直す。
        let window = app.get_window("main").expect("window registered");

        let (exit_tx, exit_rx) = channel();
        let sink: Arc<dyn PtySink> = Arc::new(ExitSink(exit_tx));
        {
            let state = app.state::<AppState>();
            state
                .pty
                .spawn_with_sink(
                    Arc::clone(&sink),
                    SpawnSpec {
                        surface_id: "window-destroyed:agent".to_string(),
                        program: "/bin/cat".to_string(),
                        args: Vec::new(),
                        cwd: std::path::PathBuf::from("/tmp"),
                        env: Vec::new(),
                        cols: DEFAULT_COLS,
                        rows: DEFAULT_ROWS,
                    },
                )
                .expect("spawn");
            assert!(state.pty.is_alive("window-destroyed:agent"));
        }

        super::kill_on_window_destroyed(&window, &WindowEvent::Destroyed);

        exit_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("WindowEvent::Destroyed must have killed the pty surface");
        assert!(
            !app.state::<AppState>()
                .pty
                .is_alive("window-destroyed:agent"),
            "kill_on_window_destroyed must kill every surface that was live when Destroyed fired"
        );
    }

    // --- M2-1 Task 6: runtime_state の Tauri 配線 ---
    //
    // ここから下の 7 本は「状態機械をアプリの寿命に結線した」ことを固定する。
    // 順序（`begin_shutdown` -> kill）は `ShutdownBegun` のフィールド private が
    // コンパイル時に固定している（逆順は `cargo build` が落ちる）ので、ここで見るのは
    // 「各経路が実際に shutdown を始めるか」「起動時正規化が何回・どの順で・失敗したら
    // どうなるか」「emit observer が実際に登録されているか」の方である。
    mod wiring {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
        use tauri::{AppHandle, Manager, WebviewWindowBuilder};

        use crate::error::{AppError, AppResult};
        use crate::model::{CliKind, RuntimeState};
        use crate::session::runtime_state::{StateInput, StatePersist};
        use crate::state::AppState;
        use crate::store::test_support::{insert_test_session, open_temp};

        /// consumer スレッドは非同期なので、条件が満たされるまで短時間待つ。
        fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                if cond() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            cond()
        }

        /// 起動時正規化の**呼ばれ方**を観測するフェイク。
        ///
        /// - `queried`: `list_ids_by_last_runtime_state` に渡された state の履歴。
        ///   同じ state が 2 回現れたら `normalize_on_startup` が 2 回走っている
        /// - `app_state_visible_during_normalize`: 走査中に `AppState` が
        ///   `manage` 済みだったか。**`true` になった時点で `sender()` に到達できる**。
        ///   読み取りフェーズ（`list_ids_by_last_runtime_state`）と書き込みフェーズ
        ///   （`set_last_runtime_state`）の**両方**で覗く。書き込みフェーズを見るために
        ///   `Running` の行を 1 件だけ返す
        /// - `fail`: 走査を `AppError::Db` で失敗させる
        struct NormalizeProbe {
            handle: AppHandle<MockRuntime>,
            queried: Mutex<Vec<RuntimeState>>,
            app_state_visible_during_normalize: AtomicBool,
            fail: bool,
        }

        impl NormalizeProbe {
            fn new(handle: AppHandle<MockRuntime>, fail: bool) -> Arc<Self> {
                Arc::new(NormalizeProbe {
                    handle,
                    queried: Mutex::new(Vec::new()),
                    app_state_visible_during_normalize: AtomicBool::new(false),
                    fail,
                })
            }

            fn queried(&self) -> Vec<RuntimeState> {
                self.queried
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
            }
        }

        impl NormalizeProbe {
            fn note_app_state_visibility(&self) {
                if self.handle.try_state::<AppState>().is_some() {
                    self.app_state_visible_during_normalize
                        .store(true, Ordering::SeqCst);
                }
            }
        }

        impl StatePersist for NormalizeProbe {
            fn set_last_runtime_state(&self, _id: &str, _state: RuntimeState) -> AppResult<()> {
                self.note_app_state_visibility();
                Ok(())
            }

            fn list_ids_by_last_runtime_state(
                &self,
                state: RuntimeState,
            ) -> AppResult<Vec<String>> {
                self.note_app_state_visibility();
                self.queried
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(state);
                if self.fail {
                    return Err(AppError::Db("boom".into()));
                }
                // 昇格する行を 1 件だけ返し、書き込みフェーズにも probe を到達させる。
                if state == RuntimeState::Running {
                    return Ok(vec!["live".to_string()]);
                }
                Ok(Vec::new())
            }

            fn mark_first_started(&self, _id: &str) -> AppResult<()> {
                Ok(())
            }

            fn set_runtime_error(&self, _id: &str, _message: &str) -> AppResult<()> {
                Ok(())
            }
        }

        fn mock_app() -> tauri::App<MockRuntime> {
            mock_builder()
                .build(mock_context(noop_assets()))
                .expect("build mock app")
        }

        /// 必達 2（PR 13 レビュー I-2）: `normalize_on_startup()` が完了するまで
        /// `sender()` を配らない。
        ///
        /// `normalize_on_startup` は読み取りフェーズを write lock の外で回してから
        /// `map.insert` を無条件に行うので、その隙間に consumer が進めた状態が
        /// DB 由来の古い値で巻き戻る。`sender()` は `&self` なので「順序を守る」だけでは
        /// 守れない —— 返した瞬間に誰でも入力を積める。`AppState` を `manage` する前に
        /// 正規化を終わらせることで、そもそも到達できなくする。
        #[test]
        fn setup_finishes_normalize_before_app_state_becomes_reachable() {
            let (_dir, store) = open_temp();
            let app = mock_app();
            let probe = NormalizeProbe::new(app.handle().clone(), false);

            super::super::install_app_state_with(&app, Arc::new(store), probe.clone());

            assert!(
                !probe
                    .app_state_visible_during_normalize
                    .load(Ordering::SeqCst),
                "正規化の走査中に AppState が manage されていた（sender() へ到達できてしまう）"
            );
            // 陽性コントロール: 走査は実際に起きており、manage も最終的には行われている
            assert!(!probe.queried().is_empty(), "正規化が一度も走っていない");
            assert!(
                app.try_state::<AppState>().is_some(),
                "install_app_state_with は最終的に AppState を manage する"
            );
        }

        /// 必達 4（Task 5 レビュー持ち越し）: `normalize_on_startup()` は
        /// `setup` でちょうど 1 回だけ呼ばれる。
        ///
        /// 冪等なので 2 回呼んでも症状が出ない。`ALL_STATES` の増減に引きずられない
        /// よう、件数ではなく「同じ state が 2 回問い合わされていないこと」で固定する。
        #[test]
        fn setup_runs_normalize_exactly_once() {
            let (_dir, store) = open_temp();
            let app = mock_app();
            let probe = NormalizeProbe::new(app.handle().clone(), false);

            super::super::install_app_state_with(&app, Arc::new(store), probe.clone());

            let queried = probe.queried();
            assert!(!queried.is_empty(), "正規化が一度も走っていない");
            let mut unique = queried.clone();
            unique.sort_by_key(|s| format!("{s:?}"));
            unique.dedup();
            assert_eq!(
                queried.len(),
                unique.len(),
                "同じ state が 2 回問い合わされている = normalize_on_startup が 2 回走った: {queried:?}"
            );
            // 昇格対象の 2 状態を見ていること（走査の主目的）
            assert!(queried.contains(&RuntimeState::Running));
            assert!(queried.contains(&RuntimeState::WaitingInput));
        }

        /// 必達 4（Task 5 レビュー持ち越し）: `AppState` に注入される
        /// `Arc<dyn StatePersist>` が**実際に `Store`** であること。
        ///
        /// フェイクを差した上のテストでは、`install_app_state`（ラッパ）が
        /// `Arc::clone(&store) as Arc<dyn StatePersist>` を渡していることまでは
        /// 見えない。実 `Store` に `running` の行を仕込み、DB を読み直して
        /// `interrupted` になっていることで、結線と正規化を同時に固定する。
        #[test]
        fn setup_normalizes_running_rows_through_the_real_store() {
            let (_dir, store) = open_temp();
            let project_id = store
                .insert_project("kamux", "/Users/x/repo/kamux", CliKind::Claude)
                .expect("insert_project")
                .id;
            let live = insert_test_session(&store, &project_id, "live");
            let resting = insert_test_session(&store, &project_id, "resting");
            store
                .set_last_runtime_state(&live.id, RuntimeState::Running)
                .expect("set_last_runtime_state");

            let app = mock_app();
            super::super::install_app_state(&app, Arc::new(store));

            let state = app.state::<AppState>();
            assert_eq!(
                state
                    .store
                    .get_session(&live.id)
                    .expect("get_session")
                    .last_runtime_state,
                RuntimeState::Interrupted,
                "running のまま終了した行は DB 上で interrupted になる"
            );
            assert_eq!(state.runtime.current(&live.id), RuntimeState::Interrupted);
            // 一度も起動していない行を ⏸ にしない（DEFAULT 'idle' の据え置き）
            assert_eq!(
                state
                    .store
                    .get_session(&resting.id)
                    .expect("get_session")
                    .last_runtime_state,
                RuntimeState::Idle
            );
        }

        /// 必達 5（Task 5 レビュー持ち越し）: 走査途中の書き込み失敗は
        /// **ログに落として起動を続行する**。
        ///
        /// 決定の理由は `install_app_state_with` の doc コメント参照。ここでは
        /// 「`Err` でも `AppState` が manage される（= アプリが起動する）」ことだけを
        /// 固定する。`?` で `setup` を失敗させる変異はここで赤くなる。
        #[test]
        fn setup_continues_when_startup_normalize_fails() {
            let (_dir, store) = open_temp();
            let app = mock_app();
            let probe = NormalizeProbe::new(app.handle().clone(), true);

            super::super::install_app_state_with(&app, Arc::new(store), probe.clone());

            assert!(!probe.queried().is_empty(), "正規化は試みられている");
            assert!(
                app.try_state::<AppState>().is_some(),
                "起動時正規化の失敗でアプリを起動させないのは過剰（契約 §0: panic 経路禁止）"
            );
        }

        /// `install_app_state_with` が `TauriEmitObserver` を **`register_observer` で
        /// 実際に登録している**こと（Task 6 レビュー I-3）。
        ///
        /// production の配線 1 行を守る唯一のテスト。
        /// `runtime_state::tests::tauri_emit_observer_emits_the_payload_on_the_contract_topic`
        /// は observer を**直接構築**するので、この 1 行を消しても緑のまま通る
        /// （レビュアーの変異 I で実測済み）。ここでは observer を 1 つも作らず、
        /// **`sender()` に入力を積んで `session://state/{id}` が届くか**だけを見る。
        ///
        /// 消えたときの症状は重い —— イベントが 1 度も発火せず、フロント（Task 8-10）と
        /// M2-2 以降が丸ごと無反応になる。
        #[test]
        fn install_app_state_registers_the_emit_observer_on_the_runtime() {
            use std::sync::mpsc::channel;

            use tauri::Listener;

            use crate::session::runtime_state::state_topic;

            let (_dir, store) = open_temp();
            let project_id = store
                .insert_project("kamux", "/x/kamux", CliKind::Claude)
                .expect("insert_project")
                .id;
            let session = insert_test_session(&store, &project_id, "live");

            let app = mock_app();
            let (tx, rx) = channel::<String>();
            app.listen(state_topic(&session.id), move |event| {
                let _ = tx.send(event.payload().to_string());
            });

            let store = Arc::new(store);
            let persist = Arc::clone(&store) as Arc<dyn StatePersist>;
            super::super::install_app_state_with(&app, store, persist);

            app.state::<AppState>()
                .runtime
                .sender()
                .send(&session.id, StateInput::Spawned);

            let raw = rx.recv_timeout(Duration::from_secs(5)).expect(
                "session://state/{id} が届かない: install_app_state_with が \
                 TauriEmitObserver を register_observer していない",
            );
            let payload: serde_json::Value = serde_json::from_str(&raw).expect("payload json");
            assert_eq!(payload["session_id"], serde_json::json!(session.id));
            assert_eq!(payload["runtime_state"], serde_json::json!("running"));
        }

        /// 「この経路は状態機械の shutdown を始めたか」を、実際の遷移で確かめる。
        ///
        /// `begin_shutdown()` の効果は「以降の入力を捨てる」ことなので、
        /// 陽性コントロール（teardown 前の 1 件が通る）と対にして初めて意味を持つ。
        /// 陽性コントロールが無いと、consumer スレッドが死んでいても緑になる。
        fn assert_path_begins_runtime_shutdown(
            app: &tauri::App<MockRuntime>,
            session_id: &str,
            drive_teardown: impl FnOnce(&tauri::App<MockRuntime>),
        ) {
            {
                let state = app.state::<AppState>();
                state.runtime.sender().send(session_id, StateInput::Spawned);
                assert!(
                    wait_until(|| state.runtime.current(session_id) == RuntimeState::Running),
                    "陽性コントロール: teardown 前は遷移が通るはず"
                );
            }

            drive_teardown(app);

            let state = app.state::<AppState>();
            // PTY kill 由来の PtyExited と同じ入力。shutdown 済みなら捨てられる。
            state
                .runtime
                .sender()
                .send(session_id, StateInput::PtyExited);
            std::thread::sleep(Duration::from_millis(50));
            assert_eq!(
                state.runtime.current(session_id),
                RuntimeState::Running,
                "teardown 後の入力はメモリを更新してはいけない"
            );
            assert_eq!(
                state
                    .store
                    .get_session(session_id)
                    .expect("get_session")
                    .last_runtime_state,
                RuntimeState::Running,
                "DB に exited が焼かれると、次回起動で ⏸ ではなく ⛔ になる（計画 §6.2）"
            );
        }

        /// 必達 1（Destroyed 側）: 窓を閉じる経路が `begin_shutdown()` を撃つ。
        ///
        /// `MockRuntime` は `WindowEvent::Destroyed` を一度も発火しないため、
        /// M1-4 と同じく関数を直接呼んで固定する（前例:
        /// `kill_on_window_destroyed_kills_every_live_pty_surface_when_called_directly`）。
        #[test]
        fn window_destroyed_begins_runtime_shutdown() {
            let (_dir, store) = open_temp();
            let project_id = store
                .insert_project("kamux", "/x/kamux", CliKind::Claude)
                .expect("insert_project")
                .id;
            let session = insert_test_session(&store, &project_id, "live");

            let app = mock_builder()
                .manage(crate::state::test_support::app_state(store))
                .build(mock_context(noop_assets()))
                .expect("build mock app");
            WebviewWindowBuilder::new(&app, "main", Default::default())
                .build()
                .expect("build webview");

            assert_path_begins_runtime_shutdown(&app, &session.id, |app| {
                let window = app.get_window("main").expect("window registered");
                super::super::kill_on_window_destroyed(&window, &tauri::WindowEvent::Destroyed);
            });
        }

        /// 必達 1（Exit 側）: Cmd+Q が通る経路が `begin_shutdown()` を撃つ。
        ///
        /// `MockRuntime` は最後の窓が消えたあと必ず `RunEvent::Exit` を発火するので、
        /// こちらは実イベントで到達する（前例: `run_event_exit_kills_every_live_pty_surface`）。
        #[test]
        fn run_event_exit_begins_runtime_shutdown() {
            use std::sync::mpsc::channel;

            let (_dir, store) = open_temp();
            let project_id = store
                .insert_project("kamux", "/x/kamux", CliKind::Claude)
                .expect("insert_project")
                .id;
            let session = insert_test_session(&store, &project_id, "live");

            let app = mock_builder()
                .manage(crate::state::test_support::app_state(store))
                .build(mock_context(noop_assets()))
                .expect("build mock app");
            let window = WebviewWindowBuilder::new(&app, "main", Default::default())
                .build()
                .expect("build webview");
            let app_handle = app.handle().clone();

            {
                let state = app_handle.state::<AppState>();
                state
                    .runtime
                    .sender()
                    .send(&session.id, StateInput::Spawned);
                assert!(
                    wait_until(|| state.runtime.current(&session.id) == RuntimeState::Running),
                    "陽性コントロール: teardown 前は遷移が通るはず"
                );
            }

            // `RunEvent::Ready` を合図に窓を閉じる（レース回避の理由は
            // `run_event_exit_kills_every_live_pty_surface` のコメント参照）。
            let (ready_tx, ready_rx) = channel::<()>();
            let run_thread = std::thread::spawn(move || {
                app.run(move |handle, event| {
                    if matches!(event, tauri::RunEvent::Ready) {
                        let _ = ready_tx.send(());
                    }
                    super::super::kill_on_run_event_exit(handle, event);
                });
            });

            ready_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("event loop must reach RunEvent::Ready before we close the window");
            window.close().expect("close the only window");
            run_thread.join().expect("event loop thread must not panic");

            let state = app_handle.state::<AppState>();
            state
                .runtime
                .sender()
                .send(&session.id, StateInput::PtyExited);
            std::thread::sleep(Duration::from_millis(50));
            assert_eq!(
                state.runtime.current(&session.id),
                RuntimeState::Running,
                "RunEvent::Exit 後の入力はメモリを更新してはいけない"
            );
            assert_eq!(
                state
                    .store
                    .get_session(&session.id)
                    .expect("get_session")
                    .last_runtime_state,
                RuntimeState::Running,
                "Cmd+Q で exited が焼かれると、次回起動で ⏸ ではなく ⛔ になる（計画 §6.2）"
            );
        }

        // --- M2-1 Task 7: 実 PTY の終了が `sink.rs` 経由で状態機械へ届く ---

        /// `TauriSink` へ委譲したあとで通知するラッパ。
        ///
        /// **`inner.on_exit` の戻り後に signal する**ので、受信できた時点で
        /// `TauriSink::on_exit`（= `note_pty_exit`）は完走している。
        /// `PtyManager::live_surfaces()` の空判定では代用できない ——
        /// `RegistrySink::on_exit` はレジストリからの削除を inner への転送より
        /// **前**に行うため、空になった時点ではまだ `note_pty_exit` が走っていない。
        struct SinkExitProbe {
            inner: Arc<dyn crate::pty::surface::PtySink>,
            tx: std::sync::mpsc::Sender<()>,
        }

        impl crate::pty::surface::PtySink for SinkExitProbe {
            fn on_data(&self, surface_id: &str, base64: String, seq: u64) {
                self.inner.on_data(surface_id, base64, seq);
            }

            fn on_exit(&self, surface_id: &str, exit_code: Option<i32>) {
                self.inner.on_exit(surface_id, exit_code);
                let _ = self.tx.send(());
            }
        }

        /// 実 PTY（`/bin/cat`）を **production と同じ `TauriSink`** で 1 本立てる。
        /// `spawn`（契約 §15）は `AppHandle<Wry>` 固定で `MockRuntime` から呼べないため、
        /// `spawn_with_sink` に同じ sink を自前で渡して同じ経路を再現する。
        fn spawn_agent_surface_through_the_real_sink(
            app: &tauri::App<MockRuntime>,
            surface_id: &str,
        ) -> std::sync::mpsc::Receiver<()> {
            use crate::pty::surface::PtySink;
            use crate::pty::{SpawnSpec, DEFAULT_COLS, DEFAULT_ROWS};

            let (tx, rx) = std::sync::mpsc::channel::<()>();
            let sink: Arc<dyn PtySink> = Arc::new(SinkExitProbe {
                inner: crate::pty::sink::TauriSink::new(app.handle().clone()),
                tx,
            });
            app.state::<AppState>()
                .pty
                .spawn_with_sink(
                    sink,
                    SpawnSpec {
                        surface_id: surface_id.to_string(),
                        program: "/bin/cat".to_string(),
                        args: Vec::new(),
                        cwd: std::path::PathBuf::from("/tmp"),
                        env: Vec::new(),
                        cols: DEFAULT_COLS,
                        rows: DEFAULT_ROWS,
                    },
                )
                .expect("spawn the agent surface for the test");
            rx
        }

        fn app_with_one_session() -> (
            tempfile::TempDir,
            tauri::App<MockRuntime>,
            crate::model::Session,
        ) {
            let (dir, store) = open_temp();
            let project_id = store
                .insert_project("kamux", "/x/kamux", CliKind::Claude)
                .expect("insert_project")
                .id;
            let session = insert_test_session(&store, &project_id, "live");
            let app = mock_builder()
                .manage(crate::state::test_support::app_state(store))
                .build(mock_context(noop_assets()))
                .expect("build mock app");
            (dir, app, session)
        }

        /// 必達 4 の陽性コントロール: `sink.rs` の `on_exit` が実 PTY の終了を
        /// 状態機械へ届け、DB に `exited` が焼かれる。
        ///
        /// `sink.rs` の `note_pty_exit` ブロックを消すと、PTY は死ぬのに
        /// `last_runtime_state` が `running` のまま残ってここが赤くなる。
        #[test]
        fn pty_exit_through_the_sink_marks_the_session_exited() {
            let (_dir, app, session) = app_with_one_session();
            let sid = format!("{}:agent", session.id);
            let exited = spawn_agent_surface_through_the_real_sink(&app, &sid);

            let state = app.state::<AppState>();
            state
                .runtime
                .sender()
                .send(&session.id, StateInput::Spawned);
            assert!(
                wait_until(|| state
                    .store
                    .get_session(&session.id)
                    .expect("get_session")
                    .last_runtime_state
                    == RuntimeState::Running),
                "陽性コントロール: spawn 前は 🟢 が DB まで届くはず"
            );

            state.pty.kill(&sid).expect("kill the agent surface");
            exited
                .recv_timeout(Duration::from_secs(10))
                .expect("TauriSink::on_exit must run within 10s of the kill");

            // 契約 §69.1: assert する対象そのもの（DB の値）を待つ
            assert!(
                wait_until(|| state
                    .store
                    .get_session(&session.id)
                    .expect("get_session")
                    .last_runtime_state
                    == RuntimeState::Exited),
                "PTY の終了が sink.rs 経由で状態機械へ届いていない"
            );
            assert_eq!(state.runtime.current(&session.id), RuntimeState::Exited);
        }

        /// 必達 5（Task 6 レビュー §6.3 の持ち越し）: **アプリ終了の一括 kill で
        /// DB に `exited` を焼かない。**
        ///
        /// `ShutdownBegun` は「`begin_shutdown()` の**後**に kill する」ことしか
        /// 強制できない —— `begin_runtime_shutdown` の本体から
        /// `runtime.begin_shutdown()` を落とし `ShutdownBegun(())` だけを返す形は
        /// 型では防げない。そこを実行時に塞ぐのがこのテストである。
        ///
        /// ここが緩むと Cmd+Q のたびに DB へ `exited` が書かれ、次回起動で
        /// `normalize_on_startup` は `{running, waiting_input}` しか触らないため
        /// 全カードが ⏸ ではなく ⛔ で復帰する（計画 §4.4 / §6.2）。
        ///
        /// Task 6 の時点では実行時の観測手段が無かった。Task 7 が
        /// `sink.rs` -> `note_pty_exit` を配線したことで、初めて
        /// 「実 PTY を殺して、状態機械に届いた入力が捨てられること」を直接見られる。
        #[test]
        fn app_teardown_does_not_persist_exited_from_the_pty_it_kills() {
            let (_dir, app, session) = app_with_one_session();
            let sid = format!("{}:agent", session.id);
            let exited = spawn_agent_surface_through_the_real_sink(&app, &sid);

            let state = app.state::<AppState>();
            state
                .runtime
                .sender()
                .send(&session.id, StateInput::Spawned);
            assert!(
                wait_until(|| state
                    .store
                    .get_session(&session.id)
                    .expect("get_session")
                    .last_runtime_state
                    == RuntimeState::Running),
                "陽性コントロール: teardown 前は 🟢 が DB まで届くはず"
            );

            super::super::shutdown_runtime_then_kill_pty(&state);

            // teardown の kill 由来の on_exit が `note_pty_exit` まで到達したことを
            // 確認する。ここを待たずに assert すると、単に「まだ届いていない」だけで
            // 緑になり、順序の保証を一切検証しないテストになる。
            exited
                .recv_timeout(Duration::from_secs(10))
                .expect("TauriSink::on_exit must run within 10s of the teardown kill");

            // 負の主張なので待つ条件が作れない（契約 §69.2）
            std::thread::sleep(Duration::from_millis(50));
            assert_eq!(
                state
                    .store
                    .get_session(&session.id)
                    .expect("get_session")
                    .last_runtime_state,
                RuntimeState::Running,
                "teardown 由来の PtyExited を DB へ焼くと、次回起動で全カードが ⛔ になる"
            );
            assert_eq!(state.runtime.current(&session.id), RuntimeState::Running);
        }
    }

    #[test]
    fn app_state_store_is_shareable_across_threads() {
        // M1-3 の PtyManager / M2-1 の SessionManager がバックグラウンドスレッドから
        // Store を触るため、Arc<Store> がスレッドを跨げることを固定する。
        let (_dir, store) = open_temp();
        let state = crate::state::test_support::app_state(store);
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
        use serde_json::{json, Value};
        use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, MockRuntime};
        use tauri::{ipc::CallbackFn, webview::InvokeRequest, Manager, WebviewWindowBuilder};

        use crate::state::AppState;
        use crate::store::test_support::open_temp;

        fn build_app(store: crate::store::Store) -> tauri::App<MockRuntime> {
            mock_builder()
                .manage(crate::state::test_support::app_state(store))
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
                    crate::session::suggest_branch_name,
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

        // resize_pty のコマンド登録と surfaceId バインドを検証する（フィックス対象
        // レビュー指摘: Task 8 fix round 2 Important 1。resize_pty は generate_handler!
        // に登録されているだけでどのテストからも invoke されていなかった）。
        // 注意: cols と rows の引数順は本テストでは弁別できない。読み戻し API が無く、
        // それを検証するには Task 6 が凍結した PtyManager 内部（cols/rows の記録）に
        // 手を入れる必要があるため、範囲外としている。
        #[test]
        fn resize_pty_reaches_the_pty_manager_and_returns_not_found_for_an_unknown_surface() {
            let (_dir, store) = open_temp();
            let app = build_app(store);
            let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
                .build()
                .expect("build webview");

            let err = invoke_err(
                &webview,
                "resize_pty",
                json!({"surfaceId": "nope:agent", "cols": 100, "rows": 40}),
            );

            assert_eq!(err["code"], json!("not_found"));
            assert_eq!(
                err["message"],
                json!("nope:agent"),
                "AppError::NotFound は surface_id をそのまま運ぶ(契約 §6)"
            );
        }

        // ack_pty のコマンド登録と surfaceId バインドを検証する（フィックス対象
        // レビュー指摘: Task 8 fix round 2 Important 1。契約 §9 のバックプレッシャー
        // 解除路であり、ここが崩れると端末が途中まで出力されて固まったまま止まる）。
        #[test]
        fn ack_pty_reaches_the_pty_manager_and_returns_not_found_for_an_unknown_surface() {
            let (_dir, store) = open_temp();
            let app = build_app(store);
            let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
                .build()
                .expect("build webview");

            let err = invoke_err(
                &webview,
                "ack_pty",
                json!({"surfaceId": "nope:agent", "seq": 1}),
            );

            assert_eq!(err["code"], json!("not_found"));
            assert_eq!(
                err["message"],
                json!("nope:agent"),
                "AppError::NotFound は surface_id をそのまま運ぶ(契約 §6)"
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

        // --- M2-1 Task 7: コマンドから状態機械への配線 ---

        /// consumer スレッドは非同期なので、条件が満たされるまで短時間待つ。
        fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while std::time::Instant::now() < deadline {
                if cond() {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            cond()
        }

        /// テストが自分で 1 本立てる agent サーフェス（`/bin/cat`）。
        /// `write_pty` は `PtyManager::write` が成功したときだけ `UserInput` を送るので、
        /// 生きた surface が要る。
        fn spawn_agent_surface(state: &AppState, agent_surface_id: &str) {
            use crate::pty::surface::PtySink;
            use crate::pty::{SpawnSpec, DEFAULT_COLS, DEFAULT_ROWS};

            struct NoopSink;
            impl PtySink for NoopSink {
                fn on_data(&self, _surface_id: &str, _base64: String, _seq: u64) {}
                fn on_exit(&self, _surface_id: &str, _exit_code: Option<i32>) {}
            }

            state
                .pty
                .spawn_with_sink(
                    std::sync::Arc::new(NoopSink),
                    SpawnSpec {
                        surface_id: agent_surface_id.to_string(),
                        program: "/bin/cat".to_string(),
                        args: Vec::new(),
                        cwd: std::path::PathBuf::from("/tmp"),
                        env: Vec::new(),
                        cols: DEFAULT_COLS,
                        rows: DEFAULT_ROWS,
                    },
                )
                .expect("spawn the agent surface directly for the test");
        }

        /// IPC 越しに project + session を 1 件ずつ作り、session_id を返す。
        fn create_session_over_ipc(webview: &tauri::WebviewWindow<MockRuntime>) -> String {
            let project = invoke_ok(
                webview,
                "create_project",
                json!({"name": "kamux", "repoPath": "/x/kamux", "defaultCli": "claude"}),
            );
            let project_id = project["id"].as_str().expect("project id").to_owned();
            let session = invoke_ok(
                webview,
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
            session["id"].as_str().expect("session id").to_owned()
        }

        /// 必達 3: `write_pty` は 🟡 を解除できる唯一の経路である。
        ///
        /// `write_pty` から `note_user_input` を落とすと、Claude が承認待ちのまま
        /// ユーザーが返事を打っても 🟡 が張り付いたままになり、ここが赤くなる。
        #[test]
        fn write_pty_notes_user_input_and_clears_waiting_input() {
            use crate::model::{RuntimeState, SurfaceKind};
            use crate::pty::surface_id;
            use crate::session::runtime_state::StateInput;

            let (_dir, store) = open_temp();
            let app = build_app(store);
            let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
                .build()
                .expect("build webview");
            let session_id = create_session_over_ipc(&webview);
            let agent_surface_id = surface_id(&session_id, SurfaceKind::Agent);

            let state = app.state::<AppState>();
            spawn_agent_surface(&state, &agent_surface_id);
            let sender = state.runtime.sender();
            sender.send(&session_id, StateInput::Spawned);
            sender.send(&session_id, StateInput::HookNotification);
            assert!(
                wait_until(|| state.runtime.current(&session_id) == RuntimeState::WaitingInput),
                "陽性コントロール: 打鍵前は 🟡 のはず"
            );

            invoke_ok(
                &webview,
                "write_pty",
                json!({"surfaceId": agent_surface_id, "data": "y"}),
            );

            assert!(
                wait_until(|| state.runtime.current(&session_id) == RuntimeState::Running),
                "write_pty が UserInput を送っていない（🟡 が解除されない）"
            );

            state.pty.kill(&agent_surface_id).expect("cleanup");
        }

        /// 必達 6: `stop_session` は kill のあと `UserStopped` を送り、⛔ を確定させる。
        ///
        /// 2 回目の停止は `PtyManager::kill` の冪等性（契約 §15）により `Ok` が返り、
        /// `UserStopped` も送られるが、`exited` + `UserStopped` は遷移なしなので
        /// DB もバッジも動かない（計画 §6.5）。
        #[test]
        fn stop_session_marks_the_session_exited_and_a_second_stop_is_harmless() {
            use crate::model::{RuntimeState, SurfaceKind};
            use crate::pty::surface_id;
            use crate::session::runtime_state::StateInput;

            let (_dir, store) = open_temp();
            let app = build_app(store);
            let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
                .build()
                .expect("build webview");
            let session_id = create_session_over_ipc(&webview);
            let agent_surface_id = surface_id(&session_id, SurfaceKind::Agent);

            let state = app.state::<AppState>();
            spawn_agent_surface(&state, &agent_surface_id);
            state
                .runtime
                .sender()
                .send(&session_id, StateInput::Spawned);
            assert!(
                wait_until(|| state
                    .store
                    .get_session(&session_id)
                    .expect("get_session")
                    .last_runtime_state
                    == RuntimeState::Running),
                "陽性コントロール: 停止前は 🟢 が DB まで届くはず"
            );

            invoke_ok(&webview, "stop_session", json!({"id": session_id}));

            // 契約 §69.1: assert する対象そのもの（DB の値）を待つ
            assert!(
                wait_until(|| state
                    .store
                    .get_session(&session_id)
                    .expect("get_session")
                    .last_runtime_state
                    == RuntimeState::Exited),
                "stop_session が UserStopped を送っていない"
            );
            assert_eq!(state.runtime.current(&session_id), RuntimeState::Exited);

            // 既に死んだセッションへの 2 発目。kill は冪等なので Ok が返る。
            invoke_ok(&webview, "stop_session", json!({"id": session_id}));

            // 負の主張なので待つ条件が作れない（契約 §69.2）
            std::thread::sleep(std::time::Duration::from_millis(50));
            assert_eq!(state.runtime.current(&session_id), RuntimeState::Exited);
            assert_eq!(
                state
                    .store
                    .get_session(&session_id)
                    .expect("get_session")
                    .last_runtime_state,
                RuntimeState::Exited
            );
        }

        // suggest_branch_name のコマンド登録と camelCase バインドを検証する（契約 §60.1）。
        // project_id を未知の値にすることで、`get_project` に到達していることと
        // 3 引数のバインドを同時に確認できる（git リポジトリ不要）。
        #[test]
        fn suggest_branch_name_binds_camel_case_and_returns_not_found_for_an_unknown_project() {
            let (_dir, store) = open_temp();
            let app = build_app(store);
            let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
                .build()
                .expect("build webview");

            let err = invoke_err(
                &webview,
                "suggest_branch_name",
                json!({"projectId": "nope", "title": "Fix login bug", "sessionId": "sess-1"}),
            );

            assert_eq!(err["code"], json!("not_found"));
            assert_eq!(
                err["message"],
                json!("nope"),
                "AppError::NotFound はキーをそのまま運ぶ(契約 §6)"
            );
        }

        // happy path: 戻り値が裸の JSON 文字列であることを固定する
        // （契約 §60.2: `BranchSuggestion { branch, slug }` は却下されている。
        // 誤って構造体へ戻す変異が入ると `.as_str()` が None になり赤くなる）。
        #[test]
        fn suggest_branch_name_returns_a_bare_string_not_an_object() {
            use crate::worktree::test_support::TestRepo;

            let repo = TestRepo::new();
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
                    "repoPath": repo.path().to_str().expect("utf8"),
                    "defaultCli": "claude",
                }),
            );
            let project_id = project["id"].as_str().expect("project id").to_owned();

            let got = invoke_ok(
                &webview,
                "suggest_branch_name",
                json!({"projectId": project_id, "title": "Fix login bug", "sessionId": "sess-1"}),
            );

            assert_eq!(got.as_str(), Some("session/fix-login-bug"), "got: {got:?}");
        }

        // `session_id` がラッパを経由して `worktree::suggest_branch_name` の第 3 引数
        // まで実際に流れていることを固定する（契約 §60.1 の採用理由 1: `id` を持たない
        // 限り §13 の fallback を作れない）。title を slug 化すると空になる値にすることで、
        // 戻り値が session_id 由来の fallback（"session-{id 先頭 8 文字}"）になることを
        // 検証する。title / session_id を取り違えるラッパの変異（第 3 引数を title に
        // 差し替える等）を上の 2 テストは弁別できないが、これは弁別する。
        #[test]
        fn suggest_branch_name_flows_session_id_into_the_fallback_slug() {
            use crate::worktree::test_support::TestRepo;

            let repo = TestRepo::new();
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
                    "repoPath": repo.path().to_str().expect("utf8"),
                    "defaultCli": "claude",
                }),
            );
            let project_id = project["id"].as_str().expect("project id").to_owned();

            let got = invoke_ok(
                &webview,
                "suggest_branch_name",
                json!({"projectId": project_id, "title": "!!!", "sessionId": "sess-1"}),
            );

            assert_eq!(got.as_str(), Some("session/session-sess-1"), "got: {got:?}");
        }
    }
}
