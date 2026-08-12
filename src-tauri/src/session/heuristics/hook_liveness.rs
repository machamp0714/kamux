//! セッション単位の hooks 疎通状態(設計書 §12「hooks 不達 → 汎用ヒューリスティックへ自動フォールバック」)。
//!
//! 猶予切れを検出するためのタイマーは持たない。ゲートが参照される瞬間に `now` から計算する。
//! 既定値（`silence_timeout_secs == DEFAULT_SILENCE_TIMEOUT_SECS == 30`）では、
//! 最初の沈黙イベントが来るのは 30 秒後で、猶予 20 秒は必ず切れているため、
//! タイマーを 1 本も増やさずに設計書 §12 の要求を満たせる。**ユーザーが
//! `silence_timeout_secs` を下限（`MIN_SILENCE_TIMEOUT_SECS == 5`）近くまで
//! 下げた場合はこの前提が成立しない** —— 沈黙イベントが猶予の中に着弾し、
//! ゲート規則 3（`hook_liveness == Pending`）で抑止される。この手当ては
//! `SessionActivity::rearm_after`（機構）と、消費ループがゲートで抑止した
//! ときにそれを呼ぶこと（方針）が持ち、本モジュールは関知しない。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use super::clock::Clock;
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

#[derive(Debug, Clone, Copy)]
struct HookLivenessEntry {
    cli_kind: CliKind,
    spawned_at_ms: i64,
    last_hook_at_ms: Option<i64>,
}

/// セッション単位の hooks 疎通記録。タイマーを持たず、問い合わせ時に判定する。
pub struct HookLivenessTracker {
    clock: Arc<dyn Clock>,
    entries: Mutex<HashMap<String, HookLivenessEntry>>,
}

impl HookLivenessTracker {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// PTY spawn 時に呼ぶ。既存エントリがあれば猶予をリセットする（resume 対応）。
    pub fn on_spawn(&self, session_id: &str, cli_kind: CliKind) {
        let now = self.clock.now_ms();
        self.lock().insert(
            session_id.to_string(),
            HookLivenessEntry {
                cli_kind,
                spawned_at_ms: now,
                last_hook_at_ms: None,
            },
        );
    }

    /// hook を受信したときに呼ぶ。`Healthy` へ単調に昇格する。
    pub fn on_hook(&self, session_id: &str) {
        let now = self.clock.now_ms();
        if let Some(entry) = self.lock().get_mut(session_id) {
            entry.last_hook_at_ms = Some(now);
        }
    }

    /// PTY 終了 / stop_session で呼ぶ。
    pub fn on_exit(&self, session_id: &str) {
        self.lock().remove(session_id);
    }

    pub fn liveness(&self, session_id: &str) -> HookLiveness {
        let now = self.clock.now_ms();
        match self.lock().get(session_id) {
            Some(e) => liveness_after_grace(e.cli_kind, e.spawned_at_ms, e.last_hook_at_ms, now),
            None => HookLiveness::NotApplicable,
        }
    }

    pub fn last_hook_at(&self, session_id: &str) -> Option<i64> {
        self.lock().get(session_id).and_then(|e| e.last_hook_at_ms)
    }

    /// 診断表示用。`(session_id, cli_kind, liveness, last_hook_at)` の一覧。
    pub fn snapshot(&self) -> Vec<(String, CliKind, HookLiveness, Option<i64>)> {
        let now = self.clock.now_ms();
        self.lock()
            .iter()
            .map(|(id, e)| {
                (
                    id.clone(),
                    e.cli_kind,
                    liveness_after_grace(e.cli_kind, e.spawned_at_ms, e.last_hook_at_ms, now),
                    e.last_hook_at_ms,
                )
            })
            .collect()
    }

    /// 毒された Mutex でも panic しない（契約 §0）
    fn lock(&self) -> MutexGuard<'_, HashMap<String, HookLivenessEntry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
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
        // `last_hook_at_ms` の値そのものは判定に使わない。0 でも「届いた事実」で
        // Healthy になる(契約 §30.4: 判定キーは「届いたか」であって値の大小ではない)。
        assert_eq!(
            liveness_after_grace(CliKind::Claude, 1_000, Some(0), 30_000),
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

#[cfg(test)]
mod tracker_tests {
    use super::*;
    use crate::session::heuristics::clock::TestClock;
    use std::sync::Arc;

    fn setup() -> (TestClock, HookLivenessTracker) {
        let clock = TestClock::new(0);
        let tracker = HookLivenessTracker::new(Arc::new(clock.clone()));
        (clock, tracker)
    }

    #[test]
    fn unknown_sessions_are_not_applicable() {
        let (_c, t) = setup();
        assert_eq!(t.liveness("never-registered"), HookLiveness::NotApplicable);
    }

    #[test]
    fn a_freshly_spawned_claude_session_is_pending() {
        let (_c, t) = setup();
        t.on_spawn("s1", CliKind::Claude);
        assert_eq!(t.liveness("s1"), HookLiveness::Pending);
    }

    #[test]
    fn a_claude_session_becomes_unreachable_after_the_grace_window() {
        let (clock, t) = setup();
        t.on_spawn("s1", CliKind::Claude);
        clock.advance_ms(HOOK_GRACE_MS);
        assert_eq!(t.liveness("s1"), HookLiveness::Unreachable);
    }

    #[test]
    fn a_hook_promotes_to_healthy() {
        let (clock, t) = setup();
        t.on_spawn("s1", CliKind::Claude);
        clock.advance_ms(1_500);
        t.on_hook("s1");
        assert_eq!(t.liveness("s1"), HookLiveness::Healthy);
        assert_eq!(t.last_hook_at("s1"), Some(1_500));
    }

    #[test]
    fn a_late_hook_promotes_from_unreachable_and_never_demotes() {
        let (clock, t) = setup();
        t.on_spawn("s1", CliKind::Claude);
        clock.advance_ms(45_000);
        assert_eq!(t.liveness("s1"), HookLiveness::Unreachable);

        t.on_hook("s1");
        assert_eq!(t.liveness("s1"), HookLiveness::Healthy);

        // 以後どれだけ hook が途絶えても降格しない（設計 §4.7 単調性）
        clock.advance_ms(3_600_000);
        assert_eq!(t.liveness("s1"), HookLiveness::Healthy);
    }

    #[test]
    fn last_hook_at_tracks_the_most_recent_hook() {
        let (clock, t) = setup();
        t.on_spawn("s1", CliKind::Claude);
        clock.advance_ms(1_000);
        t.on_hook("s1");
        clock.advance_ms(5_000);
        t.on_hook("s1");
        assert_eq!(t.last_hook_at("s1"), Some(6_000));
    }

    #[test]
    fn non_claude_sessions_stay_not_applicable() {
        let (clock, t) = setup();
        t.on_spawn("s1", CliKind::Shell);
        t.on_spawn("s2", CliKind::Codex);
        t.on_spawn("s3", CliKind::Custom);
        clock.advance_ms(600_000);
        for id in ["s1", "s2", "s3"] {
            assert_eq!(t.liveness(id), HookLiveness::NotApplicable);
        }
    }

    #[test]
    fn on_exit_forgets_the_session() {
        let (clock, t) = setup();
        t.on_spawn("s1", CliKind::Claude);
        t.on_hook("s1");
        t.on_exit("s1");
        assert_eq!(t.liveness("s1"), HookLiveness::NotApplicable);
        assert_eq!(t.last_hook_at("s1"), None);

        // 再起動したら猶予が最初からやり直しになる
        clock.advance_ms(100);
        t.on_spawn("s1", CliKind::Claude);
        assert_eq!(t.liveness("s1"), HookLiveness::Pending);
    }

    #[test]
    fn on_spawn_resets_the_grace_window() {
        let (clock, t) = setup();
        t.on_spawn("s1", CliKind::Claude);
        clock.advance_ms(HOOK_GRACE_MS);
        assert_eq!(t.liveness("s1"), HookLiveness::Unreachable);

        t.on_spawn("s1", CliKind::Claude); // resume で再 spawn
        assert_eq!(t.liveness("s1"), HookLiveness::Pending);
    }

    #[test]
    fn hooks_for_unregistered_sessions_are_ignored_without_panicking() {
        let (_c, t) = setup();
        t.on_hook("ghost");
        t.on_exit("ghost");
        assert_eq!(t.liveness("ghost"), HookLiveness::NotApplicable);
    }

    #[test]
    fn snapshot_lists_every_registered_session() {
        let (clock, t) = setup();
        t.on_spawn("s1", CliKind::Claude);
        t.on_spawn("s2", CliKind::Shell);
        t.on_hook("s1");
        clock.advance_ms(10);

        let mut snap = t.snapshot();
        snap.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(snap.len(), 2);
        assert_eq!(
            snap[0],
            (
                "s1".to_string(),
                CliKind::Claude,
                HookLiveness::Healthy,
                Some(0)
            )
        );
        assert_eq!(
            snap[1],
            (
                "s2".to_string(),
                CliKind::Shell,
                HookLiveness::NotApplicable,
                None
            )
        );
    }
}
