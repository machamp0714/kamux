// src-tauri/src/pty/sink.rs
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, Wry};

use crate::pty::surface::PtySink;

/// 契約 §8 のトピック表記。`{}` はランタイム置換
pub fn data_topic(surface_id: &str) -> String {
    format!("pty://data/{surface_id}")
}

pub fn exit_topic(surface_id: &str) -> String {
    format!("pty://exit/{surface_id}")
}

#[derive(Debug, Serialize, Clone)]
pub struct PtyDataPayload {
    /// UTF-8 不正バイトを跨ぐ分割を避けるため base64 で運ぶ
    pub base64: String,
    pub seq: u64,
}

#[derive(Debug, Serialize, Clone)]
pub struct PtyExitPayload {
    pub surface_id: String,
    pub exit_code: Option<i32>,
}

/// PTY の出力を Tauri イベントとして WebView に流す。
/// `AppHandle` を保持しているので、M2-1 はここから
/// `app.try_state::<AppState>()` 経由で状態機械に届けられる。
///
/// `R: Runtime` はテストのために足した内部実装の一般化であり、契約上の型ではない
/// （`00-contracts.md` に `TauriSink` の記載は無い）。デフォルト型パラメータが
/// `Wry` なので、production コードの `TauriSink::new(app: AppHandle)` という
/// 呼び出し方は変わらない。`tauri::test::mock_builder()` は `MockRuntime` を使う
/// ため、`R` を固定していると `pty://data` / `pty://exit` の実 emit をユニット
/// テストで検証できない（Task 8 必達 1）。
// 所有権メモ(フィックス対象レビュー指摘: Task 8 fix round 2 Minor 2)。
// AppHandle -> Arc<AppManager> -> managed AppState -> PtyManager.surfaces ->
// Arc<PtySurface> -> RegistrySink -> inner: Arc<TauriSink> -> AppHandle という
// 強参照の循環がある。on_exit で PtyManager がレジストリからエントリを外すと
// 循環が切れるので生存中の漏れ続けにはならないが、逆に言うと「プロセス終了時」は
// このパスを通らないため、PtySurface::Drop(surface.rs)はアプリ終了時には走らない。
// 終了時の子プロセス掃除は lib.rs の on_window_event 側の明示 kill に依存している。
// M2-1 が RuntimeStateManager を AppState に足すときの判断材料として残す。
// コード自体は変えない(契約 §15 の凍結範囲)。
pub struct TauriSink<R: Runtime = Wry> {
    app: AppHandle<R>,
}

impl<R: Runtime> TauriSink<R> {
    pub fn new(app: AppHandle<R>) -> Arc<Self> {
        Arc::new(Self { app })
    }
}

impl<R: Runtime> PtySink for TauriSink<R> {
    // 必ず Emitter::emit（グローバル）を使う。emit_to（webview 限定）にすると
    // Rust 側で pty://exit を購読する M2-1 の状態機械に終了が届かない
    fn on_data(&self, surface_id: &str, base64: String, seq: u64) {
        // 送信失敗（WebView 破棄など）は無視する。PTY は生かしたままにする
        let _ = self
            .app
            .emit(&data_topic(surface_id), PtyDataPayload { base64, seq });
        // M3-3: `StateInput::OutputActivity` の唯一の production 送信点（契約 §118.2）。
        //
        // **ここにフィルタを書かないこと。** `:editor` を弾く判定も、遷移しない入力を
        // 捨てる判定も `RuntimeSender::note_surface` の内部に閉じている
        // （`on_exit` の同じ注意書きを参照）。
        //
        // **ここは PTY 読み取り専用の OS スレッドである。** `note_surface` は
        // read-lock 1 回で遷移しない入力を捨て、`String` の確保もチャネル送信も
        // 行わない（契約 §118.2 の条件 2）。これ以上の処理を足さないこと。
        if let Some(state) = self.app.try_state::<crate::state::AppState>() {
            state.runtime.sender().note_surface(
                surface_id,
                crate::session::runtime_state::StateInput::OutputActivity,
            );
        }
    }

    fn on_exit(&self, surface_id: &str, exit_code: Option<i32>) {
        let _ = self.app.emit(
            &exit_topic(surface_id),
            PtyExitPayload {
                surface_id: surface_id.to_string(),
                exit_code,
            },
        );
        // M2-1: agent サーフェスの終了を状態機械へ。
        //
        // **ここにフィルタを書かないこと。** `:editor` を弾く判定は
        // `RuntimeSender::note_surface` の内部に閉じている（契約 §2）。2 箇所に散ると、
        // 片方だけ直した瞬間に「nvim を閉じただけでセッションが ⛔ になる」が戻る。
        //
        // **ここは PTY 読み取り専用の OS スレッドである。ブロックすると PTY の
        // 読み取りそのものが止まる。** `note_pty_exit` は `Mutex` を取って
        // チャネルへ push するだけで、DB 書き込みは consumer スレッドが行う。
        // これ以上の処理を足さないこと。
        //
        // `try_state` が `None` を返す場合（アプリ終了処理中に起こりうる）は黙って
        // 何もしない —— `begin_shutdown()` 後の遷移を捨てる方針と一致する。
        //
        // M2-4 / 契約 §41.3 決定 (3): 終了理由を `ResumeTracker::classify_exit` で
        // 分類し、`note_pty_exit` / `note_resume_failed_exit` を出し分ける。
        // **`classify_exit` は resume 試行フラグを消費するので、1 終了につき
        // 1 回だけ呼ぶ。** `:editor` の弾きは `classify_exit` と note_* の両方が
        // 持つ（前者は試行フラグを消費しないため、後者は状態機械の入口のため）。
        // ここで import するのは `StateReason`（比較用）だけで、`StateInput` は
        // import しない（M2-1 Task 7 の規約）。
        if let Some(state) = self.app.try_state::<crate::state::AppState>() {
            let sender = state.runtime.sender();
            if state.resume_tracker.classify_exit(surface_id, exit_code)
                == crate::model::StateReason::ResumeFailed
            {
                sender.note_resume_failed_exit(surface_id);
            } else {
                sender.note_pty_exit(surface_id);
            }
            // M3-3: ヒューリスティックの登録を外す。**production で唯一の PTY 終了経路**
            // なので、ここに置かないと終了したセッションのウォッチャが残る。
            //
            // 🔴 **上の 2 つの注意書き（「ここにフィルタを書かないこと」）はここには
            // 掛からない。** あれは `RuntimeSender::note_surface` の内部に閉じた
            // *状態機械の入力* フィルタの話である。レジストリは `session_id` を
            // キーにしていて `note_surface` 相当の入口を持たないので、
            // agent 限定の判定は呼び出し側が行うしかない。極性を手書きで散らさないよう、
            // 判定そのものは `pty::agent_session_id`（純関数）1 箇所に置いてある。
            //
            // `s1:editor` の終了で `unregister("s1")` を呼ぶと、nvim を閉じただけで
            // agent 側の沈黙推定が死ぬ。`unregister` は未知 ID で no-op なので、
            // この誤りはコンパイルでも黙って通る。
            if let Some(session_id) = crate::pty::agent_session_id(surface_id) {
                crate::session::heuristics::sink_impl::detach_heuristics(
                    &state.heuristics,
                    session_id,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topics_match_the_contract_spelling() {
        assert_eq!(
            data_topic("3f2a9c1e-0000-4000-8000-000000000001:agent"),
            "pty://data/3f2a9c1e-0000-4000-8000-000000000001:agent"
        );
        assert_eq!(
            exit_topic("3f2a9c1e-0000-4000-8000-000000000001:editor"),
            "pty://exit/3f2a9c1e-0000-4000-8000-000000000001:editor"
        );
    }

    #[test]
    fn data_payload_serializes_with_contract_field_names() {
        let json = serde_json::to_string(&PtyDataPayload {
            base64: "aGVsbG8=".to_string(),
            seq: 42,
        })
        .expect("serialize");
        assert_eq!(json, r#"{"base64":"aGVsbG8=","seq":42}"#);
    }

    #[test]
    fn exit_payload_serializes_with_contract_field_names() {
        let json = serde_json::to_string(&PtyExitPayload {
            surface_id: "s1:agent".to_string(),
            exit_code: Some(0),
        })
        .expect("serialize");
        assert_eq!(json, r#"{"surface_id":"s1:agent","exit_code":0}"#);

        let json_null = serde_json::to_string(&PtyExitPayload {
            surface_id: "s1:agent".to_string(),
            exit_code: None,
        })
        .expect("serialize");
        assert_eq!(json_null, r#"{"surface_id":"s1:agent","exit_code":null}"#);
    }

    // --- 必達 1: pty://data / pty://exit が実際に emit されることを検証する ---
    //
    // `Listener::listen` はハードコードのトピック文字列で登録する。`data_topic(sid)` を
    // 使うと `data_topic` 自体が壊れても両辺が一緒に動いて緑のまま残ってしまうため。
    mod emits {
        use std::sync::mpsc::{channel, RecvTimeoutError};
        use std::time::Duration;

        use tauri::test::{mock_builder, mock_context, noop_assets};
        use tauri::Listener;

        use super::*;

        #[test]
        fn on_data_emits_a_global_event_on_the_contract_data_topic() {
            let app = mock_builder()
                .build(mock_context(noop_assets()))
                .expect("build mock app");
            let handle = app.handle().clone();

            // PtyDataPayload に Deserialize を生やすと契約型がテスト都合で太る
            // ため、生の JSON 値としてフィールドを読む
            let (tx, rx) = channel::<serde_json::Value>();
            handle.listen("pty://data/test:agent", move |event| {
                let payload: serde_json::Value =
                    serde_json::from_str(event.payload()).expect("deserialize payload");
                let _ = tx.send(payload);
            });

            let sink = TauriSink::new(handle);
            sink.on_data("test:agent", "aGVsbG8=".to_string(), 7);

            let received = rx
                .recv_timeout(Duration::from_secs(5))
                .expect("pty://data event must be emitted within 5s");
            assert_eq!(received["base64"], serde_json::json!("aGVsbG8="));
            assert_eq!(received["seq"], serde_json::json!(7));
        }

        #[test]
        fn on_exit_emits_a_global_event_on_the_contract_exit_topic() {
            let app = mock_builder()
                .build(mock_context(noop_assets()))
                .expect("build mock app");
            let handle = app.handle().clone();

            let (tx, rx) = channel::<serde_json::Value>();
            handle.listen("pty://exit/test:agent", move |event| {
                let payload: serde_json::Value =
                    serde_json::from_str(event.payload()).expect("deserialize payload");
                let _ = tx.send(payload);
            });

            let sink = TauriSink::new(handle);
            sink.on_exit("test:agent", Some(0));

            let received = rx
                .recv_timeout(Duration::from_secs(5))
                .expect("pty://exit event must be emitted within 5s");
            assert_eq!(received["surface_id"], serde_json::json!("test:agent"));
            assert_eq!(received["exit_code"], serde_json::json!(0));
        }

        // 未登録のトピックへは届かないことを見て、上 2 つの positive テストが
        // 「たまたま何か受信できた」で緑になっていないことの対照。
        // emit_to への差し替え検出そのものは上 2 つの positive テストが担う
        // （emit_to は webview 不在では届かず negative 側は元々タイムアウトで
        // 緑のままなので、この negative テスト単体は emit_to を弁別しない）
        #[test]
        fn on_data_does_not_emit_to_a_different_surfaces_topic() {
            let app = mock_builder()
                .build(mock_context(noop_assets()))
                .expect("build mock app");
            let handle = app.handle().clone();

            let (tx, rx) = channel::<()>();
            handle.listen("pty://data/other:agent", move |_event| {
                let _ = tx.send(());
            });

            let sink = TauriSink::new(handle);
            sink.on_data("test:agent", "aGVsbG8=".to_string(), 1);

            match rx.recv_timeout(Duration::from_millis(200)) {
                Err(RecvTimeoutError::Timeout) => {}
                other => panic!("unexpected event on unrelated topic: {other:?}"),
            }
        }
    }

    // --- M2-4: 終了理由の出し分け（契約 §41.3 決定 (3)）---
    //
    // `PtyExited` と `ResumeFailed` は**どちらも `RuntimeState::Exited` へ落ちる**
    // （§41.3 決定 (1) の表は `PtyExited` 列の逐語コピーである）。したがって
    // `state.runtime.current(&id)` を見るテストは、実装が両方を `note_pty_exit` に
    // 潰していても緑になる。**差が出るのは `StateReason` だけなので、observer から
    // reason を直接読む。**
    mod resume_failure {
        use std::sync::Mutex;
        use std::time::Duration;

        use tauri::test::{mock_builder, mock_context, noop_assets};
        use tauri::Manager;

        use super::*;
        use crate::model::{CliKind, SessionStatePayload, StateReason};
        use crate::session::cli_args::ResumePlan;
        use crate::session::runtime_state::{StateInput, StateObserver};
        use crate::state::AppState;
        use crate::store::test_support::{insert_test_session, open_temp};

        #[derive(Default)]
        struct RecordingObserver {
            seen: Mutex<Vec<(String, StateReason)>>,
        }

        impl StateObserver for RecordingObserver {
            fn on_state(&self, payload: &SessionStatePayload) {
                self.seen
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((payload.session_id.clone(), payload.reason));
            }
        }

        impl RecordingObserver {
            /// PTY 終了に由来する理由だけを残す。fixture の `Spawned`
            /// （`StateReason::Spawned`）は memory 更新 → DB 書き込み → observer 通知の
            /// 順で非同期に進むため、絞らないと位置指定の assert が残留レースで壊れる
            /// （`hooks_srv/handler.rs` の `hook_reasons` と同じ手当て）。
            fn exit_reasons(&self) -> Vec<(String, StateReason)> {
                self.seen
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                    .filter(|(_, reason)| {
                        matches!(reason, StateReason::PtyExited | StateReason::ResumeFailed)
                    })
                    .cloned()
                    .collect()
            }

            /// consumer スレッド経由の通知が届くまで有界時間だけ待つ。
            fn wait_for_one_exit_reason(&self) -> Vec<(String, StateReason)> {
                for _ in 0..200 {
                    let seen = self.exit_reasons();
                    if !seen.is_empty() {
                        return seen;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                panic!("no exit-originated state change observed within 2s");
            }
        }

        /// `on_exit` が `ResumeTracker::classify_exit` の結果で
        /// `note_resume_failed_exit` / `note_pty_exit` を出し分けていること。
        ///
        /// 陽性の対照は下の
        /// `an_untracked_nonzero_exit_is_reported_as_a_plain_pty_exit` が持つ ——
        /// **同じ `on_exit(surface, Some(1))` の呼び出しで、resume 試行の有無だけが
        /// 違う。** 片側だけだと、分岐が効いているのか、常に片方を呼んでいるだけ
        /// なのかを区別できない。
        #[test]
        fn a_nonzero_exit_of_a_tracked_resume_attempt_is_reported_as_resume_failed() {
            let (_dir, store) = open_temp();
            let project = store
                .insert_project("kamux", "/tmp/kamux-test-repo", CliKind::Claude)
                .expect("insert project");
            let session = insert_test_session(&store, &project.id, "resume");

            let app = mock_builder()
                .manage(crate::state::test_support::app_state(store))
                .build(mock_context(noop_assets()))
                .expect("build mock app");
            let handle = app.handle().clone();
            let state = handle.state::<AppState>();

            // 陽性の対照（前提）: 遷移が通る状態から測る。`Idle` のまま測ると
            // `Exited` への遷移そのものは起きるが、fixture が本当に生きていることを
            // 確かめられない。
            state
                .runtime
                .sender()
                .send(&session.id, StateInput::Spawned);
            let observer = Arc::new(RecordingObserver::default());
            state.runtime.register_observer(observer.clone());
            state
                .resume_tracker
                .mark_resume_attempt(&session.id, &ResumePlan::ClaudeContinue);

            TauriSink::new(handle.clone()).on_exit(&format!("{}:agent", session.id), Some(1));

            assert_eq!(
                observer.wait_for_one_exit_reason(),
                vec![(session.id.clone(), StateReason::ResumeFailed)],
                "resume 試行中の非ゼロ終了は ResumeFailed として状態機械へ届くこと"
            );
            state.runtime.begin_shutdown();
        }

        /// 陽性の対照。resume 試行が記録されていない終了は素の `PtyExited` のまま。
        #[test]
        fn an_untracked_nonzero_exit_is_reported_as_a_plain_pty_exit() {
            let (_dir, store) = open_temp();
            let project = store
                .insert_project("kamux", "/tmp/kamux-test-repo", CliKind::Claude)
                .expect("insert project");
            let session = insert_test_session(&store, &project.id, "fresh");

            let app = mock_builder()
                .manage(crate::state::test_support::app_state(store))
                .build(mock_context(noop_assets()))
                .expect("build mock app");
            let handle = app.handle().clone();
            let state = handle.state::<AppState>();

            state
                .runtime
                .sender()
                .send(&session.id, StateInput::Spawned);
            let observer = Arc::new(RecordingObserver::default());
            state.runtime.register_observer(observer.clone());

            TauriSink::new(handle.clone()).on_exit(&format!("{}:agent", session.id), Some(1));

            assert_eq!(
                observer.wait_for_one_exit_reason(),
                vec![(session.id.clone(), StateReason::PtyExited)],
                "resume を試みていない終了に ResumeFailed を付けてはならない"
            );
            state.runtime.begin_shutdown();
        }
    }

    // --- 必達 3: PtyManager drop 後(終了済み AppHandle)への emit が panic しないこと ---
    mod does_not_panic_after_app_teardown {
        use tauri::test::{mock_builder, mock_context, noop_assets};

        use super::*;

        #[test]
        fn on_exit_and_on_data_do_not_panic_after_the_app_is_dropped() {
            let app = mock_builder()
                .build(mock_context(noop_assets()))
                .expect("build mock app");
            let handle = app.handle().clone();
            drop(app);

            let sink = TauriSink::new(handle);
            // どちらも panic せず完走すれば緑。
            //
            // 実測メモ: `let _ = self.app.emit(...)` を `.expect("emit")` に変異
            // させてもこのテストは緑のままだった(MockRuntime では App を drop
            // しても AppHandle 経由の emit が Err にならない)。つまりこのテストは
            // 「emit が実際に失敗する経路」を検出できておらず、`let _ = ...` の
            // 形であることの担保はコードレビューに委ねている。ここで固定して
            // いるのは「on_exit/on_data がどんな AppHandle の状態でも panic
            // しない」という、より弱いが実測で赤くできる性質のみ
            // (on_exit 本体に panic を仕込むと実測で赤くなることを確認済み)。
            sink.on_exit("torn-down:agent", Some(1));
            sink.on_data("torn-down:agent", "AA==".to_string(), 1);
        }
    }
}
