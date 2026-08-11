//! セッション単位の hooks 疎通状態(設計書 §12「hooks 不達 → 汎用ヒューリスティックへ自動フォールバック」)。
//!
//! 猶予切れを検出するためのタイマーは持たない。ゲートが参照される瞬間に `now` から計算する。
//! 最初の沈黙イベントが来るのは既定 30 秒後で、猶予 20 秒は必ず切れているため、
//! タイマーを 1 本も増やさずに設計書 §12 の要求を満たせる。

use serde::{Deserialize, Serialize};

use super::HOOK_GRACE_MS;
use crate::model::CliKind;

/// セッション単位の hooks 疎通状態。`Healthy` へは単調にしか遷移しない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookLiveness {
    /// hooks を持たない CLI(codex / shell / custom)
    NotApplicable,
    /// claude セッション起動直後。まだ hook を待っている
    Pending,
    /// hook を 1 度以上受信した。ヒューリスティックは抑止される
    Healthy,
    /// 猶予時間内に hook が届かなかった。ヒューリスティックへフォールバックする
    Unreachable,
}

/// 猶予時間の経過と hook 受信履歴から疎通状態を導出する。
pub fn liveness_after_grace(
    cli_kind: CliKind,
    spawned_at_ms: i64,
    last_hook_at_ms: Option<i64>,
    now_ms: i64,
) -> HookLiveness {
    // 契約 §30.4: cli_kind の判定より前に置くこと。shim(契約 §30)があると
    // cli_kind == Shell にも hook が届くため、cli_kind を先に見ると
    // last_hook_at が記録済みでも NotApplicable が返り、§4.8 のゲート ルール 2
    // (hook 由来が常に勝つ)が張られない
    if last_hook_at_ms.is_some() {
        // 一度でも届いたら降格しない。ソケットが死ぬのは全セッション同時に起きる事象で、
        // セッション単位で降格させると状態がバタつく(設計 §4.7)
        return HookLiveness::Healthy;
    }
    if cli_kind != CliKind::Claude {
        return HookLiveness::NotApplicable;
    }
    if now_ms - spawned_at_ms >= HOOK_GRACE_MS {
        HookLiveness::Unreachable
    } else {
        HookLiveness::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_claude_clis_are_not_applicable() {
        for kind in [CliKind::Codex, CliKind::Shell, CliKind::Custom] {
            assert_eq!(
                liveness_after_grace(kind, 0, None, 999_999),
                HookLiveness::NotApplicable,
                "{kind:?} は hook が 1 度も届いていなければ hooks 対象外"
            );
        }
    }

    /// 契約 §30.4: shim 経由で shell セッションにも hook が届きうる。
    /// 届いた事実がある以上、cli_kind に関わらず Healthy でなければ
    /// §4.8 のゲート ルール 2(hook 由来が常に勝つ)が張られない
    #[test]
    fn a_shell_session_that_received_a_hook_is_healthy() {
        assert_eq!(
            liveness_after_grace(CliKind::Shell, 0, Some(1_500), 2_000),
            HookLiveness::Healthy
        );
        assert_eq!(
            liveness_after_grace(CliKind::Custom, 0, Some(1_500), 999_999),
            HookLiveness::Healthy
        );
    }

    #[test]
    fn claude_is_pending_inside_the_grace_window() {
        assert_eq!(
            liveness_after_grace(CliKind::Claude, 0, None, 0),
            HookLiveness::Pending
        );
        assert_eq!(
            liveness_after_grace(CliKind::Claude, 0, None, 19_999),
            HookLiveness::Pending
        );
    }

    #[test]
    fn claude_becomes_unreachable_exactly_at_the_grace_boundary() {
        assert_eq!(
            liveness_after_grace(CliKind::Claude, 0, None, 20_000),
            HookLiveness::Unreachable
        );
    }

    #[test]
    fn claude_stays_unreachable_long_after_the_grace_window() {
        assert_eq!(
            liveness_after_grace(CliKind::Claude, 0, None, 600_000),
            HookLiveness::Unreachable
        );
    }

    #[test]
    fn a_single_hook_promotes_to_healthy_even_inside_the_grace_window() {
        assert_eq!(
            liveness_after_grace(CliKind::Claude, 0, Some(1_500), 2_000),
            HookLiveness::Healthy
        );
    }

    #[test]
    fn a_late_hook_promotes_from_unreachable_to_healthy() {
        // フォールバック後に hook が届いたケース(設計 §4.7)
        assert_eq!(
            liveness_after_grace(CliKind::Claude, 0, Some(45_000), 46_000),
            HookLiveness::Healthy
        );
    }

    #[test]
    fn grace_is_measured_from_spawn_not_from_zero() {
        assert_eq!(
            liveness_after_grace(CliKind::Claude, 1_000_000, None, 1_010_000),
            HookLiveness::Pending
        );
        assert_eq!(
            liveness_after_grace(CliKind::Claude, 1_000_000, None, 1_020_000),
            HookLiveness::Unreachable
        );
    }

    #[test]
    fn grace_window_is_shorter_than_the_default_silence_timeout() {
        // この不変条件が崩れると、hook を待つ前に沈黙推定が走ってしまう(設計 §4.7)
        assert!(HOOK_GRACE_MS < (super::super::DEFAULT_SILENCE_TIMEOUT_SECS as i64) * 1_000);
    }

    #[test]
    fn serializes_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&HookLiveness::NotApplicable).unwrap(),
            "\"not_applicable\""
        );
        assert_eq!(
            serde_json::to_string(&HookLiveness::Pending).unwrap(),
            "\"pending\""
        );
        assert_eq!(
            serde_json::to_string(&HookLiveness::Healthy).unwrap(),
            "\"healthy\""
        );
        assert_eq!(
            serde_json::to_string(&HookLiveness::Unreachable).unwrap(),
            "\"unreachable\""
        );
    }
}
