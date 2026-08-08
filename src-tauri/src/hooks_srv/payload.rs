//! hook payload のデシリアライズ。
//!
//! 契約 §12.5 の方針: kamux は payload の共通部分しか読まない。
//! さらに契約 §84.1 で **必須フィールドを 1 つも置かない** ことが正典化された。
//! `Notification` / `PermissionRequest` の payload 構造が未確認（契約 §12.4）なため、
//! 必須フィールドがあるとフィールド欠落でデシリアライズ全体が失敗し、
//! イベントが丸ごと落ちるリスクがある。
//!
//! 「デシリアライズが失敗しえない」構造にした代わりに、契約 §84.1.1 が義務化した
//! 3 経路 + §91.1.1 が追加した条件 1-4（内側のデシリアライズ失敗）の計 4 経路の
//! `tracing::warn!` を [`parse_hook_event`] の中に持つ。

use serde::Deserialize;

/// relay → hooks_srv のワイヤ形式（契約 §84.3）。
#[derive(Debug, Clone, Deserialize)]
pub struct WireMessage {
    pub v: u32,
    pub kamux_session_id: String,
    /// hook 種別。settings JSON の argv 第 1 引数由来（契約 §84.1）。
    pub hook_kind: String,
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
    /// payload が JSON として読めなかったときの生 stdin（先頭 4 KiB まで）。デバッグ用。
    #[serde(default)]
    pub raw_base64: Option<String>,
    /// relay が stdin の読み取り上限に当たった。このとき payload は必ず null。
    #[serde(default)]
    pub truncated: bool,
}

/// 全 hook イベント共通。未知フィールドは無視する（deny_unknown_fields を付けない）。
/// 必須フィールドは存在しない（契約 §84.1）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HookEnvelope {
    #[serde(default)]
    pub hook_event_name: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// SessionStart のみ: "startup" / "resume" 等
    #[serde(default)]
    pub source: Option<String>,
    /// Stop のみ
    #[serde(default)]
    pub stop_hook_active: Option<bool>,
    /// 上記以外の全フィールドを保持。Notification の構造が判明するまでの受け皿。
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// hook の種別。argv 第 1 引数から決まる。payload には依存しない。
///
/// kamux が登録するのは 4 種（契約 §12.4）。それ以外は Other に落として無視する。
/// ユーザー自身の settings.json とマージされる（契約 §12.2）ので、
/// 登録していないイベントが届くことは想定内。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookKind {
    SessionStart,
    Notification,
    /// パーミッションダイアログ表示時。waiting_input の最も直接的な信号（契約 §12.4）
    PermissionRequest,
    Stop,
    Other(String),
}

impl HookKind {
    pub fn from_argv(s: &str) -> Self {
        match s {
            "SessionStart" => HookKind::SessionStart,
            "Notification" => HookKind::Notification,
            "PermissionRequest" => HookKind::PermissionRequest,
            "Stop" => HookKind::Stop,
            other => HookKind::Other(other.to_string()),
        }
    }
}

/// hooks_srv が state 機械へ渡す唯一の型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookEvent {
    /// kamux の Session.id。env の KAMUX_SESSION_ID 由来
    pub kamux_session_id: String,
    pub kind: HookKind,
    /// payload の session_id。--resume 用（契約 §12.6）。取れないことがある
    pub claude_session_id: Option<String>,
    /// SessionStart の "startup" / "resume" 等。記録用
    pub source: Option<String>,
}

/// ワイヤ上のバイト列を HookEvent に変換する。
/// WireMessage 自体の破損だけがエラーになる。payload の中身は何であっても成功する。
pub fn parse_hook_event(bytes: &[u8]) -> Result<HookEvent, serde_json::Error> {
    let wire: WireMessage = serde_json::from_slice(bytes)?;
    let kind = HookKind::from_argv(&wire.hook_kind);

    // 契約 §84.1.1 条件 1-2: relay が stdin を JSON として解釈できなかった
    // （raw_base64 が付く）か、stdin の読み取り上限に当たった（truncated）。
    // hook_kind は argv 由来なので状態遷移は続くが、claude_session_id / source は
    // 必ず None になる。これが鳴っていたら payload 側の情報は一切拾えていない。
    if wire.payload.is_none() {
        tracing::warn!(
            kamux_session_id = %wire.kamux_session_id,
            hook_kind = %wire.hook_kind,
            truncated = wire.truncated,
            raw_base64 = wire.raw_base64.as_deref().unwrap_or(""),
            "hook payload is null; relay could not parse stdin as JSON"
        );
    }

    let envelope: HookEnvelope = match wire.payload {
        Some(value) => {
            let raw_json = value.to_string();
            // 契約 §91.1.1 条件 1-4: payload は Some だが HookEnvelope への
            // デシリアライズが失敗した（map ではない、または既知フィールドの型が
            // 変わった）。§84.1 が「失敗しえない」のは外側（ワイヤ → WireMessage /
            // Value の受け取り）だけであり、内側（Value → HookEnvelope）は今も
            // 失敗しうる。§12.5 の原規則（デシリアライズ失敗は握りつぶさず
            // warn! に生 JSON を残す）がこの層でまだ生きているので、
            // unwrap_or_default() で握りつぶさず、記録してから既定値へ落とす。
            serde_json::from_value(value).unwrap_or_else(|err| {
                tracing::warn!(
                    kamux_session_id = %wire.kamux_session_id,
                    error = %err,
                    raw_json = %raw_json,
                    "hook payload could not be deserialized into HookEnvelope; falling back to an empty envelope"
                );
                HookEnvelope::default()
            })
        }
        None => HookEnvelope::default(),
    };

    // 契約 §84.1.1 条件 2: argv の hook_kind と payload.hook_event_name が食い違う。
    // 分岐には argv (kind) を使い続ける。ここは記録するだけで、調停ロジック
    // （どちらを優先するか決める処理）は作らない。
    if let Some(payload_name) = envelope.hook_event_name.as_deref() {
        if payload_name != wire.hook_kind {
            tracing::warn!(
                kamux_session_id = %wire.kamux_session_id,
                argv_hook_kind = %wire.hook_kind,
                payload_hook_event_name = payload_name,
                "argv hook_kind and payload.hook_event_name disagree; argv wins"
            );
        }
    }

    // 契約 §84.1.1 条件 1-1（最重要）: SessionStart なのに payload.session_id が
    // 取れない。--continue は新しい session_id を発行する（契約 §12.6）ため、ここで
    // 沈黙すると「claude_session_id が書き戻されず、次回以降も永遠に --continue へ
    // フォールバックし続ける」経路を無言で作ることになる。
    if kind == HookKind::SessionStart && envelope.session_id.is_none() {
        tracing::warn!(
            kamux_session_id = %wire.kamux_session_id,
            raw_json = %String::from_utf8_lossy(bytes),
            "SessionStart hook arrived without payload.session_id; claude_session_id will not be persisted"
        );
    }

    Ok(HookEvent {
        kamux_session_id: wire.kamux_session_id,
        claude_session_id: envelope.session_id,
        source: envelope.source,
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex, OnceLock};

    /// `tracing::warn!` が実際に鳴ったことを検証するために記録する 1 イベント。
    ///
    /// レビューで指摘された通り、`fmt` の文字列出力を `contains` で見る形は
    /// 恒真になりうる（例: 条件 1-1 の `raw_json` フィールドには wire 全体が
    /// 含まれるため、`kamux_session_id` を落とす変異でも `contains` が通る）。
    /// ここではイベントをフィールド単位で構造化して記録し、フィールド値を
    /// 個別に assert する。
    #[derive(Debug, Clone)]
    struct CapturedEvent {
        level: tracing::Level,
        message: String,
        fields: BTreeMap<String, String>,
    }

    type Sink = Arc<Mutex<Vec<CapturedEvent>>>;

    thread_local! {
        /// 現在のスレッドが収集中のイベント置き場。`None` なら捨てる。
        static CAPTURE_SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
    }

    struct Visitor<'a>(&'a mut CapturedEvent);

    impl tracing::field::Visit for Visitor<'_> {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            if field.name() == "message" {
                self.0.message = value.to_string();
            } else {
                self.0
                    .fields
                    .insert(field.name().to_string(), value.to_string());
            }
        }

        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            let rendered = format!("{value:?}");
            if field.name() == "message" {
                self.0.message = rendered;
            } else {
                self.0.fields.insert(field.name().to_string(), rendered);
            }
        }
    }

    /// プロセスに 1 つだけ立てる `Subscriber`。イベントは呼び出し元スレッドの
    /// [`CAPTURE_SINK`] へ流し、sink が無いスレッドでは捨てる。
    ///
    /// **なぜ `with_default`（スレッドローカルの scoped dispatcher）ではないか。**
    /// `tracing` の callsite ごとの `Interest` は**プロセス大域**にキャッシュされる。
    /// キャッシュは callsite の初回登録時に計算され、そのとき
    /// `tracing_core::callsite::Rebuilder::JustOne` 経路では
    /// `dispatcher::get_default` —— つまり**登録した瞬間のスレッドの** subscriber
    /// —— が使われる。`parse_hook_event` は capture を張らないテスト
    /// （`argv_kind_wins_over_payload_hook_event_name` など）からも呼ばれるため、
    /// そのスレッドが callsite の初回登録を取ると `NoSubscriber::register_callsite`
    /// が返す `Interest::never()` が焼き付き、以後 capture 側へイベントが 1 件も
    /// 届かなくなる（並列実行下で間欠に発生。実測 2/60）。
    ///
    /// 大域 subscriber を 1 つ立てておくと `get_default` は常にこれを返すので
    /// `never` が計算されることが無い。さらにテスト側が `Dispatch` を作らなく
    /// なるため、dispatcher の登録・破棄に伴う競合そのものが消える。
    struct GlobalCaptureSubscriber;

    impl tracing::Subscriber for GlobalCaptureSubscriber {
        fn register_callsite(
            &self,
            _meta: &'static tracing::Metadata<'static>,
        ) -> tracing::subscriber::Interest {
            tracing::subscriber::Interest::always()
        }

        fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
            Some(tracing::level_filters::LevelFilter::TRACE)
        }

        fn enabled(&self, _meta: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn event(&self, event: &tracing::Event<'_>) {
            CAPTURE_SINK.with(|cell| {
                let borrowed = cell.borrow();
                let Some(sink) = borrowed.as_ref() else {
                    return;
                };
                let mut captured = CapturedEvent {
                    level: *event.metadata().level(),
                    message: String::new(),
                    fields: BTreeMap::new(),
                };
                event.record(&mut Visitor(&mut captured));
                sink.lock().expect("capture sink").push(captured);
            });
        }

        // span は使わないので受け取るだけ。Id は 0 が禁止されている。
        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
        fn enter(&self, _span: &tracing::span::Id) {}
        fn exit(&self, _span: &tracing::span::Id) {}
    }

    /// 大域 subscriber をプロセスで 1 回だけ立てる。
    /// 失敗を握りつぶすとキャプチャが黙って空を返す（= 否定テストが恒真になる）ので、
    /// ここで必ず落とす。
    fn install_capture_subscriber() {
        static INSTALLED: OnceLock<Result<(), String>> = OnceLock::new();
        let installed = INSTALLED.get_or_init(|| {
            tracing::subscriber::set_global_default(GlobalCaptureSubscriber)
                .map_err(|err| err.to_string())
        });
        assert!(
            installed.is_ok(),
            "global capture subscriber must be installed: {installed:?}"
        );
    }

    /// このスレッドの sink を差し込み、抜けるときに必ず外す。
    struct SinkGuard;

    impl SinkGuard {
        fn install(sink: Sink) -> Self {
            CAPTURE_SINK.with(|cell| *cell.borrow_mut() = Some(sink));
            SinkGuard
        }
    }

    impl Drop for SinkGuard {
        fn drop(&mut self) {
            CAPTURE_SINK.with(|cell| *cell.borrow_mut() = None);
        }
    }

    /// `f` を実行しつつ、その間に鳴った `tracing` イベントをすべて集めて返す。
    fn capture_events<T>(f: impl FnOnce() -> T) -> (T, Vec<CapturedEvent>) {
        install_capture_subscriber();
        let sink: Sink = Arc::new(Mutex::new(Vec::new()));
        let guard = SinkGuard::install(Arc::clone(&sink));
        let result = f();
        drop(guard);
        let events = sink.lock().expect("capture sink").clone();
        (result, events)
    }

    /// 契約 §12.4 の実測済み SessionStart payload（逐語）。
    const SESSION_START_PAYLOAD: &str = r#"{
  "session_id": "550e8400-e29b-41d4-a716-446655440000",
  "transcript_path": "/Users/user/.claude/projects/...",
  "cwd": "/path/to/working/directory",
  "hook_event_name": "SessionStart",
  "source": "startup"
}"#;

    /// 契約 §12.4 の実測済み Stop payload（逐語）。
    const STOP_PAYLOAD: &str = r#"{
  "session_id": "550e8400-...",
  "transcript_path": "...",
  "cwd": "...",
  "prompt_id": "5fe4bd0f-...",
  "permission_mode": "auto",
  "effort": { "level": "high" },
  "hook_event_name": "Stop",
  "stop_hook_active": false,
  "last_assistant_message": "...",
  "background_tasks": [],
  "session_crons": []
}"#;

    fn wire(hook_kind: &str, payload: &str) -> Vec<u8> {
        wire_with_id(KAMUX_ID, hook_kind, payload)
    }

    const KAMUX_ID: &str = "3f2a0000-0000-4000-8000-000000009c1e";
    /// 「鳴ってはいけない側」の入力に付ける ID。否定テストで、鳴った 1 件が
    /// どちらの入力に由来するかを区別するために使う。
    const QUIET_ID: &str = "00000000-0000-4000-8000-000000000000";
    /// 「鳴るべき対照側」の入力に付ける ID。
    const LOUD_ID: &str = "11111111-1111-4111-8111-111111111111";

    fn wire_with_id(kamux_session_id: &str, hook_kind: &str, payload: &str) -> Vec<u8> {
        format!(
            r#"{{"v":1,"kamux_session_id":"{kamux_session_id}","hook_kind":"{hook_kind}","payload":{payload}}}"#
        )
        .into_bytes()
    }

    /// 否定の主張（「この入力では鳴らない」）専用のヘルパ。
    ///
    /// `quiet` だけを流して `events.is_empty()` を見る形は、キャプチャ機構が
    /// 何らかの理由でイベントを 1 件も返さなくなった瞬間に**恒真**になる
    /// （契約 §69.2: 負の主張が壊れる向きは偽陰性で、CI を赤くしない）。
    /// そこで同じ capture の中で、**同じ `warn!` callsite を鳴らす対照入力** `loud`
    /// も流し、鳴った 1 件が `loud` 由来であることまで固定する。
    ///
    /// これで次の 4 つがすべて赤になる:
    /// - キャプチャが空を返す / `warn!` ブロックの削除 → 0 件
    /// - 条件の反転 → `quiet` 側が鳴り、`kamux_session_id` が `QUIET_ID` になる
    /// - 対照の callsite が死んでいる → 0 件
    fn assert_only_the_loud_input_warns(quiet: &[u8], loud: &[u8]) {
        let (_, events) = capture_events(|| {
            parse_hook_event(quiet).expect("quiet input must parse");
            parse_hook_event(loud).expect("loud input must parse");
        });

        assert_eq!(
            events.len(),
            1,
            "expected exactly one warn (from the loud control input), got {events:?}"
        );
        assert_eq!(
            events[0].fields.get("kamux_session_id").map(String::as_str),
            Some(LOUD_ID),
            "the warn must come from the loud control input, not the quiet one: {:?}",
            events[0]
        );
    }

    #[test]
    fn parses_real_session_start_payload() {
        let ev = parse_hook_event(&wire("SessionStart", SESSION_START_PAYLOAD)).expect("parse");
        assert_eq!(ev.kamux_session_id, "3f2a0000-0000-4000-8000-000000009c1e");
        assert_eq!(ev.kind, HookKind::SessionStart);
        assert_eq!(
            ev.claude_session_id.as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
        assert_eq!(ev.source.as_deref(), Some("startup"));
    }

    #[test]
    fn parses_real_stop_payload_and_ignores_extra_fields() {
        let ev = parse_hook_event(&wire("Stop", STOP_PAYLOAD)).expect("parse");
        assert_eq!(ev.kind, HookKind::Stop);
        assert_eq!(ev.claude_session_id.as_deref(), Some("550e8400-..."));
        assert_eq!(ev.source, None);
    }

    #[test]
    fn session_start_resume_source_is_preserved() {
        let payload = r#"{"session_id":"abc","hook_event_name":"SessionStart","source":"resume"}"#;
        let ev = parse_hook_event(&wire("SessionStart", payload)).expect("parse");
        assert_eq!(ev.source.as_deref(), Some("resume"));
        assert_eq!(ev.claude_session_id.as_deref(), Some("abc"));
    }

    /// 契約 §12.4: Notification / PermissionRequest の payload 構造は未確認。
    /// 構造がどうであっても kind は argv 由来なので必ず正しく決まる。
    #[test]
    fn notification_kind_comes_from_argv_not_payload() {
        let ev = parse_hook_event(&wire(
            "Notification",
            r#"{"totally":"unknown","shape":[1,2,3]}"#,
        ))
        .expect("parse");
        assert_eq!(ev.kind, HookKind::Notification);
        assert_eq!(ev.claude_session_id, None);
    }

    /// 契約 §12.4 で追加された権限系 hook。payload 構造は完全に未知。
    #[test]
    fn permission_request_kind_comes_from_argv_too() {
        let ev = parse_hook_event(&wire(
            "PermissionRequest",
            r#"{"shape":"entirely unknown"}"#,
        ))
        .expect("parse");
        assert_eq!(ev.kind, HookKind::PermissionRequest);
        assert_eq!(ev.claude_session_id, None);
    }

    /// PermissionDenied は登録しないので Other に落ちる（= 無視される）。
    #[test]
    fn permission_denied_falls_through_to_other() {
        let ev = parse_hook_event(&wire("PermissionDenied", r#"{}"#)).expect("parse");
        assert_eq!(ev.kind, HookKind::Other("PermissionDenied".to_string()));
    }

    #[test]
    fn null_payload_still_yields_an_event() {
        let bytes = br#"{"v":1,"kamux_session_id":"3f2a0000-0000-4000-8000-000000009c1e","hook_kind":"Notification","payload":null,"raw_base64":"bm90IGpzb24="}"#;
        let ev = parse_hook_event(bytes).expect("parse");
        assert_eq!(ev.kind, HookKind::Notification);
        assert_eq!(ev.claude_session_id, None);
        assert_eq!(ev.source, None);
    }

    /// relay が stdin を切り詰めても、kind は argv 由来なので状態遷移は成立する。
    #[test]
    fn truncated_message_still_yields_the_right_kind() {
        let bytes = br#"{"v":1,"kamux_session_id":"3f2a0000-0000-4000-8000-000000009c1e","hook_kind":"Stop","payload":null,"raw_base64":"eHh4","truncated":true}"#;
        let wire: WireMessage = serde_json::from_slice(bytes).expect("wire parses");
        assert!(wire.truncated);

        let ev = parse_hook_event(bytes).expect("parse");
        assert_eq!(ev.kind, HookKind::Stop);
    }

    /// payload の hook_event_name が argv と食い違っても argv を正とする。
    #[test]
    fn argv_kind_wins_over_payload_hook_event_name() {
        let payload = r#"{"session_id":"abc","hook_event_name":"SomethingElse"}"#;
        let ev = parse_hook_event(&wire("Stop", payload)).expect("parse");
        assert_eq!(ev.kind, HookKind::Stop);
    }

    #[test]
    fn unknown_hook_kind_becomes_other() {
        let ev = parse_hook_event(&wire("PreToolUse", r#"{"session_id":"abc"}"#)).expect("parse");
        assert_eq!(ev.kind, HookKind::Other("PreToolUse".to_string()));
    }

    #[test]
    fn broken_wire_message_is_an_error_not_a_panic() {
        assert!(parse_hook_event(b"garbage").is_err());
        assert!(parse_hook_event(b"").is_err());
        // 契約 §84.3: `v` を読まずに捨てる実装にはしない。`v` が欠落した wire は
        // WireMessage のデシリアライズ自体が失敗すること（レビュー Important 4）。
        assert!(
            parse_hook_event(br#"{"kamux_session_id":"k","hook_kind":"Stop","payload":{}}"#)
                .is_err()
        );
    }

    /// 契約 §84.1.1 条件 1-1（最重要）: `SessionStart` なのに `payload.session_id` が
    /// `None` のとき、`tracing::warn!` が実際に鳴ること。
    ///
    /// レビュー Important 1: 既存の `wire("SessionStart", …)` テストは 2 本とも
    /// payload に `session_id` を持っており、この分岐に入る入力が 1 本も無かった。
    #[test]
    fn session_start_without_session_id_logs_warning() {
        let bytes = wire(
            "SessionStart",
            r#"{"hook_event_name":"SessionStart","source":"startup"}"#,
        );
        let (ev, events) = capture_events(|| parse_hook_event(&bytes).expect("parse"));

        assert_eq!(ev.claude_session_id, None);
        assert_eq!(events.len(), 1, "expected exactly one warn, got {events:?}");
        assert_eq!(events[0].level, tracing::Level::WARN);
        assert_eq!(
            events[0].fields.get("kamux_session_id").map(String::as_str),
            Some("3f2a0000-0000-4000-8000-000000009c1e")
        );
        assert!(
            events[0]
                .message
                .contains("SessionStart hook arrived without payload.session_id"),
            "unexpected message: {}",
            events[0].message
        );
        // 契約 §84.1.1 条件 1: `raw_json` に生 JSON が実際に載っていること。
        // wire にしか無い `kamux_session_id` と payload 由来の `source":"startup` の
        // 両方を見ることで、フィールド削除（生 JSON が空になる）と payload だけを
        // 積む取り違えの両方を落とす。
        let raw_json = events[0].fields.get("raw_json").map(String::as_str);
        assert!(
            raw_json.is_some_and(
                |s| s.contains(r#""kamux_session_id""#) && s.contains(r#""source":"startup""#)
            ),
            "raw_json missing wire+payload markers: {raw_json:?}"
        );
    }

    /// `SessionStart` かつ `payload.session_id` が取れているときは条件 1-1 は鳴らない
    /// （条件反転の変異を塞ぐ）。
    #[test]
    fn session_start_with_session_id_does_not_log_condition_1_1() {
        let quiet = wire_with_id(QUIET_ID, "SessionStart", SESSION_START_PAYLOAD);
        // 対照: 同じ条件 1-1 の callsite を鳴らす入力（session_id が無い SessionStart）。
        let loud = wire_with_id(
            LOUD_ID,
            "SessionStart",
            r#"{"hook_event_name":"SessionStart","source":"startup"}"#,
        );
        assert_only_the_loud_input_warns(&quiet, &loud);
    }

    /// 契約 §84.1.1 条件 1-2: `payload` が `null` のとき `tracing::warn!` が実際に鳴ること。
    ///
    /// レビュー Important 2: `null_payload_still_yields_an_event` は分岐を通過するだけで
    /// `warn!` が鳴ったかを検査していなかった。
    #[test]
    fn null_payload_logs_warning() {
        let bytes = br#"{"v":1,"kamux_session_id":"3f2a0000-0000-4000-8000-000000009c1e","hook_kind":"Notification","payload":null,"raw_base64":"bm90IGpzb24="}"#;
        let (ev, events) = capture_events(|| parse_hook_event(bytes).expect("parse"));

        assert_eq!(ev.kind, HookKind::Notification);
        assert_eq!(events.len(), 1, "expected exactly one warn, got {events:?}");
        assert_eq!(events[0].level, tracing::Level::WARN);
        assert_eq!(
            events[0].fields.get("kamux_session_id").map(String::as_str),
            Some("3f2a0000-0000-4000-8000-000000009c1e")
        );
        assert_eq!(
            events[0].fields.get("truncated").map(String::as_str),
            Some("false")
        );
        assert!(
            events[0].message.contains("hook payload is null"),
            "unexpected message: {}",
            events[0].message
        );
        // 契約 §84.1.1 条件 1-2: `raw_base64` に生データが実際に載っていること。
        assert_eq!(
            events[0].fields.get("raw_base64").map(String::as_str),
            Some("bm90IGpzb24=")
        );
    }

    /// 契約 §84.1.1 条件 2: argv の `hook_kind` と `payload.hook_event_name` が食い違う
    /// とき `tracing::warn!` が実際に鳴り、両方の値が個別に記録されること。
    ///
    /// レビュー Important 2: `argv_kind_wins_over_payload_hook_event_name` は分岐を通過
    /// するだけで `warn!` が鳴ったかを検査していなかった。
    #[test]
    fn argv_payload_mismatch_logs_warning_with_both_values() {
        let payload = r#"{"session_id":"abc","hook_event_name":"SomethingElse"}"#;
        let bytes = wire("Stop", payload);
        let (ev, events) = capture_events(|| parse_hook_event(&bytes).expect("parse"));

        assert_eq!(ev.kind, HookKind::Stop);
        assert_eq!(events.len(), 1, "expected exactly one warn, got {events:?}");
        assert_eq!(events[0].level, tracing::Level::WARN);
        assert_eq!(
            events[0].fields.get("argv_hook_kind").map(String::as_str),
            Some("Stop")
        );
        assert_eq!(
            events[0]
                .fields
                .get("payload_hook_event_name")
                .map(String::as_str),
            Some("SomethingElse")
        );
    }

    /// argv と `payload.hook_event_name` が一致するときは条件 2 は鳴らない
    /// （条件反転の変異を塞ぐ）。
    #[test]
    fn argv_payload_match_does_not_log_condition_2() {
        let quiet = wire_with_id(QUIET_ID, "Stop", STOP_PAYLOAD);
        // 対照: 同じ条件 2 の callsite を鳴らす入力（argv と hook_event_name が不一致）。
        let loud = wire_with_id(
            LOUD_ID,
            "Stop",
            r#"{"session_id":"abc","hook_event_name":"SomethingElse"}"#,
        );
        assert_only_the_loud_input_warns(&quiet, &loud);
    }

    /// 契約 §84.1: `payload` の中身は何であっても成功する（object でなくても良い）。
    /// 契約 §91.1.1 条件 1-4: このとき `HookEnvelope` へのデシリアライズは失敗するので
    /// `tracing::warn!` が実際に鳴ること。
    ///
    /// レビュー Important 3: `unwrap_or_default()` を `?` に変える変異が無検出だった。
    /// `payload.rs:92` の doc コメント「payload の中身は何であっても成功する」を固定する。
    #[test]
    fn non_object_payload_is_discarded_and_logs_warning() {
        let bytes = br#"{"v":1,"kamux_session_id":"3f2a0000-0000-4000-8000-000000009c1e","hook_kind":"Notification","payload":[1,2,3]}"#;
        let (ev, events) =
            capture_events(|| parse_hook_event(bytes).expect("parse must still succeed"));

        assert_eq!(ev.claude_session_id, None);
        assert_eq!(events.len(), 1, "expected exactly one warn, got {events:?}");
        assert_eq!(events[0].level, tracing::Level::WARN);
        assert_eq!(
            events[0].fields.get("kamux_session_id").map(String::as_str),
            Some("3f2a0000-0000-4000-8000-000000009c1e")
        );
        assert!(
            events[0]
                .message
                .contains("could not be deserialized into HookEnvelope"),
            "unexpected message: {}",
            events[0].message
        );
        // 契約 §91.1.1 は逐語で「生 JSON を warn! に残す」と定めている。raw_json を
        // 落とす変異は上のアサートだけでは無検出になるため、値まで固定する。
        assert_eq!(
            events[0].fields.get("raw_json").map(String::as_str),
            Some("[1,2,3]")
        );
    }

    /// 契約 §91.1.1: `payload` が object であっても、既知フィールドの型が違えば
    /// デシリアライズは失敗する（`!is_object()` では取りこぼす具体例）。
    /// `hook_event_name` は `Option<String>` なので数値は入らない。
    #[test]
    fn object_payload_with_wrong_field_type_is_discarded_and_logs_warning() {
        let bytes = br#"{"v":1,"kamux_session_id":"3f2a0000-0000-4000-8000-000000009c1e","hook_kind":"Notification","payload":{"hook_event_name":42}}"#;
        let (ev, events) =
            capture_events(|| parse_hook_event(bytes).expect("parse must still succeed"));

        assert_eq!(ev.claude_session_id, None);
        assert_eq!(events.len(), 1, "expected exactly one warn, got {events:?}");
        assert_eq!(events[0].level, tracing::Level::WARN);
        assert!(
            events[0]
                .message
                .contains("could not be deserialized into HookEnvelope"),
            "unexpected message: {}",
            events[0].message
        );
        assert_eq!(
            events[0].fields.get("raw_json").map(String::as_str),
            Some(r#"{"hook_event_name":42}"#)
        );
    }

    /// `payload` が正しく `HookEnvelope` へデシリアライズできるときは条件 1-4 は鳴らない
    /// （条件反転の変異を塞ぐ）。
    #[test]
    fn well_formed_payload_does_not_log_condition_1_4() {
        let quiet = wire_with_id(QUIET_ID, "Stop", STOP_PAYLOAD);
        // 対照: 同じ条件 1-4 の callsite を鳴らす入力（object でない payload）。
        let loud = wire_with_id(LOUD_ID, "Notification", "[1,2,3]");
        assert_only_the_loud_input_warns(&quiet, &loud);
    }

    /// HookEnvelope は必須フィールドを持たないので、フィールドの欠落では失敗しない。
    /// ただし既知フィールドの型が違えば失敗する（契約 §91.1.1。
    /// `object_payload_with_wrong_field_type_is_discarded_and_logs_warning` 参照）。
    #[test]
    fn envelope_has_no_required_fields() {
        let empty: HookEnvelope = serde_json::from_str("{}").expect("empty object must parse");
        assert_eq!(empty.hook_event_name, None);
        assert_eq!(empty.session_id, None);
        assert!(empty.extra.is_empty());

        let odd: HookEnvelope = serde_json::from_str(r#"{"a":1,"b":[true]}"#).expect("must parse");
        assert_eq!(odd.extra.len(), 2);
    }
}
