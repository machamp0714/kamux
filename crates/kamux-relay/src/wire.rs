//! relay → hooks_srv のワイヤ形式。
//! 1 コネクション = 1 メッセージ。常に valid JSON になることを serde_json の構築で保証する。

use base64::Engine as _;

pub const WIRE_VERSION: u32 = 1;

/// UUID の `8-4-4-4-12` レイアウト（各セグメントは hex 桁のみ）で構成されているかを検査する。
///
/// ハイフンの位置まで見るのは、位置がずれた 36 文字の文字列（総文字数だけを見ると
/// 通ってしまう）を弾くため。
pub fn is_valid_session_id(s: &str) -> bool {
    const SEGMENT_LENGTHS: [usize; 5] = [8, 4, 4, 4, 12];

    let segments: Vec<&str> = s.split('-').collect();
    if segments.len() != SEGMENT_LENGTHS.len() {
        return false;
    }
    segments
        .iter()
        .zip(SEGMENT_LENGTHS.iter())
        .all(|(segment, &len)| {
            segment.len() == len && segment.chars().all(|c| c.is_ascii_hexdigit())
        })
}

/// 解析不能だった生 stdin をデバッグ用に運ぶ上限。これ以上は捨てる。
const RAW_DEBUG_CAP: usize = 4 * 1024;

/// stdin の生バイト列を解釈せずに包む。
///
/// JSON として読めれば `payload` に埋め、読めなければ `payload: null` + `raw_base64`。
/// `truncated` は stdin が読み取り上限に当たったことを示す。切り詰められた JSON は
/// まず必ずパースに失敗するので、原因が分かるようフラグを立てて運ぶ。
pub fn build_wire_message(
    kamux_session_id: &str,
    hook_kind: &str,
    raw_stdin: &[u8],
    truncated: bool,
) -> String {
    let mut obj = serde_json::Map::new();
    obj.insert("v".into(), serde_json::json!(WIRE_VERSION));
    obj.insert(
        "kamux_session_id".into(),
        serde_json::json!(kamux_session_id),
    );
    obj.insert("hook_kind".into(), serde_json::json!(hook_kind));
    if truncated {
        obj.insert("truncated".into(), serde_json::json!(true));
    }

    let parsed = if truncated {
        None
    } else {
        serde_json::from_slice::<serde_json::Value>(raw_stdin).ok()
    };

    match parsed {
        Some(value) => {
            obj.insert("payload".into(), value);
        }
        None => {
            obj.insert("payload".into(), serde_json::Value::Null);
            let head = &raw_stdin[..raw_stdin.len().min(RAW_DEBUG_CAP)];
            let encoded = base64::engine::general_purpose::STANDARD.encode(head);
            obj.insert("raw_base64".into(), serde_json::json!(encoded));
        }
    }

    // serde_json::to_string は改行を含まない。to_string_pretty は使わない。
    serde_json::Value::Object(obj).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_valid_payload_as_json_value() {
        let raw = br#"{"session_id":"550e8400-e29b-41d4-a716-446655440000","hook_event_name":"Stop","stop_hook_active":false}"#;
        let s = build_wire_message("3f2a0000-0000-4000-8000-000000009c1e", "Stop", raw, false);

        let v: serde_json::Value =
            serde_json::from_str(&s).expect("wire message must be valid JSON");
        assert_eq!(v["v"], 1);
        assert_eq!(
            v["kamux_session_id"],
            "3f2a0000-0000-4000-8000-000000009c1e"
        );
        assert_eq!(v["hook_kind"], "Stop");
        assert_eq!(v["payload"]["hook_event_name"], "Stop");
        assert_eq!(
            v["payload"]["session_id"],
            "550e8400-e29b-41d4-a716-446655440000"
        );
        assert!(v.get("raw_base64").is_none());
    }

    #[test]
    fn broken_payload_still_produces_valid_json() {
        let s = build_wire_message(
            "3f2a0000-0000-4000-8000-000000009c1e",
            "Notification",
            b"not json at all",
            false,
        );

        let v: serde_json::Value =
            serde_json::from_str(&s).expect("wire message must be valid JSON");
        assert_eq!(v["hook_kind"], "Notification");
        assert!(v["payload"].is_null());
        assert_eq!(v["raw_base64"], "bm90IGpzb24gYXQgYWxs");
    }

    #[test]
    fn empty_stdin_still_produces_valid_json() {
        let s = build_wire_message(
            "3f2a0000-0000-4000-8000-000000009c1e",
            "Notification",
            b"",
            false,
        );

        let v: serde_json::Value =
            serde_json::from_str(&s).expect("wire message must be valid JSON");
        assert!(v["payload"].is_null());
        assert_eq!(v["raw_base64"], "");
    }

    #[test]
    fn truncated_input_is_flagged_and_base64_is_capped() {
        let raw = vec![b'x'; 10_000];
        let s = build_wire_message("3f2a0000-0000-4000-8000-000000009c1e", "Stop", &raw, true);

        let v: serde_json::Value =
            serde_json::from_str(&s).expect("wire message must be valid JSON");
        assert_eq!(v["truncated"], true);
        assert!(v["payload"].is_null());
        // 契約 §84.3: 先頭 4 KiB(4096 バイト) まで。base64 にすると 5464 文字ちょうど。
        // 片側境界（< 6000 など）だと縮める方向の変異（RAW_DEBUG_CAP を 1 KiB にする等）
        // を捕まえられないため、両側を閉じた値で固定する。
        let encoded = v["raw_base64"].as_str().expect("raw_base64");
        assert_eq!(
            encoded.len(),
            5464,
            "raw_base64 must be capped to exactly 4 KiB (5464 base64 chars), got {}",
            encoded.len()
        );
    }

    #[test]
    fn untruncated_messages_have_no_truncated_key() {
        let s = build_wire_message("3f2a0000-0000-4000-8000-000000009c1e", "Stop", b"{}", false);
        let v: serde_json::Value = serde_json::from_str(&s).expect("valid JSON");
        assert!(v.get("truncated").is_none());
    }

    #[test]
    fn message_is_a_single_line() {
        let raw = b"{\n  \"session_id\": \"a\"\n}";
        let s = build_wire_message("3f2a0000-0000-4000-8000-000000009c1e", "Stop", raw, false);
        assert_eq!(
            s.lines().count(),
            1,
            "wire message must not contain newlines: {s}"
        );
    }

    #[test]
    fn session_id_validation() {
        assert!(is_valid_session_id("3f2a0000-0000-4000-8000-000000009c1e"));
        assert!(!is_valid_session_id(""));
        assert!(!is_valid_session_id("short"));
        assert!(!is_valid_session_id("3f2a0000-0000-4000-8000-000000009c1")); // 35 文字
        assert!(!is_valid_session_id("3f2a0000-0000-4000-8000-00000000-c1e")); // 非 hex
    }

    #[test]
    fn rejects_non_hex_characters_with_correct_layout() {
        // レイアウト（8-4-4-4-12、ハイフン位置）は正しいが、最後のセグメントに
        // hex ではない文字 `z` を含む。レイアウト検査だけでは弾けない。
        assert!(!is_valid_session_id("3f2a0000-0000-4000-8000-00000000zz1e"));
    }

    #[test]
    fn truncated_forces_null_payload_even_when_json_is_valid() {
        // truncated: true のときは、たとえ stdin が有効な JSON としてパースできても
        // 中身を見ずに payload を null にする（切り詰めで壊れたデータを誤って
        // 完全なものとして扱わないため）。
        let raw = br#"{"a":1}"#;
        let s = build_wire_message("3f2a0000-0000-4000-8000-000000009c1e", "Stop", raw, true);

        let v: serde_json::Value =
            serde_json::from_str(&s).expect("wire message must be valid JSON");
        assert!(v["payload"].is_null());
    }
}
