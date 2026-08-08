//! hook payload のデシリアライズ。
//!
//! 契約 §12.5 の方針: kamux は payload の共通部分しか読まない。
//! さらに契約 §84.1 で **必須フィールドを 1 つも置かない** ことが正典化された。
//! `Notification` / `PermissionRequest` の payload 構造が未確認（契約 §12.4）なため、
//! 必須フィールドがあるとフィールド欠落でデシリアライズ全体が失敗し、
//! イベントが丸ごと落ちるリスクがある。
//!
//! 「デシリアライズが失敗しえない」構造にした代わりに、契約 §84.1.1 が義務化した
//! 3 経路の `tracing::warn!` を [`parse_hook_event`] の中に持つ。

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
        Some(value) => serde_json::from_value(value).unwrap_or_default(),
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
        format!(
            r#"{{"v":1,"kamux_session_id":"3f2a0000-0000-4000-8000-000000009c1e","hook_kind":"{hook_kind}","payload":{payload}}}"#
        )
        .into_bytes()
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
    }

    /// HookEnvelope は必須フィールドを持たないので、いかなる JSON オブジェクトでもパースできる。
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
