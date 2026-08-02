// src-tauri/src/pty/sink.rs
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime, Wry};

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
    }

    fn on_exit(&self, surface_id: &str, exit_code: Option<i32>) {
        let _ = self.app.emit(
            &exit_topic(surface_id),
            PtyExitPayload {
                surface_id: surface_id.to_string(),
                exit_code,
            },
        );
        // M2-1 がここに次の 3 行を足す（`use tauri::Manager;` が要る）。
        // agent サーフェスの判定は note_pty_exit の内部に閉じているので、
        // ここでは surface_id をそのまま渡す。
        //
        // if let Some(state) = self.app.try_state::<crate::state::AppState>() {
        //     state.runtime.sender().note_pty_exit(surface_id);
        // }
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
