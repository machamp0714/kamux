//! 設定値の検証。範囲外は丸めずに拒否し、UI がユーザーへ理由を出せるようにする。

use super::{MAX_SILENCE_TIMEOUT_SECS, MIN_SILENCE_TIMEOUT_SECS};
use crate::error::{AppError, AppResult};

/// 沈黙タイムアウト（秒）を検証する。範囲は `5..=3600`。
///
/// 下限 5 秒は、0 を許すとウォッチャが busy loop になるのを構造的に防ぐため。
/// `clamp_timeout_secs` は内部の保険であり、ユーザー入力はここで明示的に拒否する。
pub fn validate_silence_timeout_secs(secs: u32) -> AppResult<u32> {
    if (MIN_SILENCE_TIMEOUT_SECS..=MAX_SILENCE_TIMEOUT_SECS).contains(&secs) {
        Ok(secs)
    } else {
        Err(AppError::InvalidState(format!(
            "silence_timeout_secs は {MIN_SILENCE_TIMEOUT_SECS}..={MAX_SILENCE_TIMEOUT_SECS} 秒の範囲で指定してください（指定値: {secs}）"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AppError;

    #[test]
    fn accepts_the_documented_default() {
        assert_eq!(validate_silence_timeout_secs(30).unwrap(), 30);
    }

    #[test]
    fn accepts_both_boundaries() {
        assert_eq!(
            validate_silence_timeout_secs(MIN_SILENCE_TIMEOUT_SECS).unwrap(),
            5
        );
        assert_eq!(
            validate_silence_timeout_secs(MAX_SILENCE_TIMEOUT_SECS).unwrap(),
            3600
        );
    }

    #[test]
    fn rejects_zero_with_invalid_state() {
        match validate_silence_timeout_secs(0) {
            Err(AppError::InvalidState(msg)) => {
                assert!(msg.contains("silence_timeout_secs"), "メッセージ: {msg}");
                assert!(
                    msg.contains('5') && msg.contains("3600"),
                    "範囲を示すこと: {msg}"
                );
            }
            other => panic!("InvalidState を期待したが {other:?}"),
        }
    }

    #[test]
    fn rejects_below_the_minimum() {
        assert!(validate_silence_timeout_secs(4).is_err());
    }

    #[test]
    fn rejects_above_the_maximum() {
        assert!(validate_silence_timeout_secs(3601).is_err());
    }

    #[test]
    fn session_patch_carries_the_new_fields() {
        let patch: crate::model::SessionPatch =
            serde_json::from_str(r#"{"heuristics_enabled": false, "silence_timeout_secs": 120}"#)
                .expect("deserialize");
        assert_eq!(patch.heuristics_enabled, Some(false));
        assert_eq!(patch.silence_timeout_secs, Some(120));
    }

    #[test]
    fn session_patch_omitting_the_new_fields_leaves_them_none() {
        let patch: crate::model::SessionPatch =
            serde_json::from_str(r#"{"title": "renamed"}"#).expect("deserialize");
        assert_eq!(patch.heuristics_enabled, None);
        assert_eq!(patch.silence_timeout_secs, None);
    }
}
