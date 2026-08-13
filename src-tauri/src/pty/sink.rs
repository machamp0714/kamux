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
        // M2-4 Task 8 がここを `ResumeTracker::classify_exit(surface_id, exit_code)`
        // による分岐（`note_pty_exit` / `note_resume_failed_exit`）へ置き換える
        // （契約 §41.3 決定 (3)）。M2-1 では `ResumeTracker` がまだ無い。
        if let Some(state) = self.app.try_state::<crate::state::AppState>() {
            state.runtime.sender().note_pty_exit(surface_id);
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
