//! 沈黙タイムアウト判定。時刻は全て引数で受け取る純粋関数なので、
//! 実時間を 1 ミリ秒も待たずに境界値を網羅できる。

/// ウォッチャが次に取るべき行動。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SilenceStep {
    /// あと `ms` ミリ秒待ってから再評価する。`timeout_ms >= 1` の呼び出しでは `ms` は必ず 1 以上
    /// （`timeout_ms == 0` を渡した場合のみ `ms == 0` になり得る）。この関数自体は
    /// `timeout_ms` をクランプしないため、`timeout_ms == 0` を渡さないことは呼び出し側の
    /// 責務である（唯一の呼び出し元は `SessionActivity::watch_silence` で、その
    /// `silence_timeout_ms` は `registry::clamp_timeout_secs` が
    /// `MIN_SILENCE_TIMEOUT_SECS` 以上へ丸めた値である）
    Wait { ms: u64 },
    /// 沈黙が成立した。イベントを送ってウォッチャを終了する
    Fire,
}

/// 最終出力活動からの経過時間で発火可否を決める。
///
/// - `now_ms <= last_activity_ms`（同時刻・時計の巻き戻し）は満額待ち直す。
///   NTP 補正で時計が戻ったときに誤発火しないため。
pub fn silence_step(now_ms: i64, last_activity_ms: i64, timeout_ms: u64) -> SilenceStep {
    let elapsed = now_ms - last_activity_ms;
    if elapsed <= 0 {
        return SilenceStep::Wait { ms: timeout_ms };
    }

    let elapsed = elapsed as u64;
    if elapsed >= timeout_ms {
        SilenceStep::Fire
    } else {
        SilenceStep::Wait {
            ms: timeout_ms - elapsed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u64 = 30_000;

    #[test]
    fn waits_the_full_timeout_right_after_activity() {
        assert_eq!(
            silence_step(1_000, 1_000, T),
            SilenceStep::Wait { ms: 30_000 }
        );
    }

    #[test]
    fn waits_only_the_remainder_when_partially_elapsed() {
        assert_eq!(
            silence_step(1_000 + 29_000, 1_000, T),
            SilenceStep::Wait { ms: 1_000 }
        );
    }

    #[test]
    fn waits_one_ms_at_the_boundary_minus_one() {
        assert_eq!(silence_step(29_999, 0, T), SilenceStep::Wait { ms: 1 });
    }

    #[test]
    fn fires_exactly_at_the_timeout() {
        assert_eq!(silence_step(30_000, 0, T), SilenceStep::Fire);
    }

    #[test]
    fn fires_when_well_past_the_timeout() {
        assert_eq!(silence_step(999_999, 0, T), SilenceStep::Fire);
    }

    #[test]
    fn clock_going_backwards_waits_the_full_timeout() {
        // NTP 補正などで now < last になった場合。発火してはいけない
        assert_eq!(
            silence_step(500, 5_000, T),
            SilenceStep::Wait { ms: 30_000 }
        );
    }

    #[test]
    fn same_instant_waits_the_full_timeout() {
        assert_eq!(silence_step(7, 7, T), SilenceStep::Wait { ms: 30_000 });
    }

    #[test]
    fn respects_a_custom_short_timeout() {
        assert_eq!(silence_step(0, 0, 5_000), SilenceStep::Wait { ms: 5_000 });
        assert_eq!(silence_step(5_000, 0, 5_000), SilenceStep::Fire);
    }

    #[test]
    fn respects_a_custom_long_timeout() {
        assert_eq!(
            silence_step(0, 0, 3_600_000),
            SilenceStep::Wait { ms: 3_600_000 }
        );
        assert_eq!(
            silence_step(3_599_999, 0, 3_600_000),
            SilenceStep::Wait { ms: 1 }
        );
        assert_eq!(silence_step(3_600_000, 0, 3_600_000), SilenceStep::Fire);
    }

    #[test]
    fn zero_timeout_waits_zero_instead_of_firing() {
        // timeout_ms == 0 をクランプせず渡さないのは呼び出し側の責務（Wait{ms:0} の
        // doc コメントに書いた前提条件そのもの）。この関数自体は elapsed <= 0 の
        // 早期 return を必ず経由し、Fire にはならない。
        assert_eq!(silence_step(0, 0, 0), SilenceStep::Wait { ms: 0 });
    }

    #[test]
    fn never_returns_a_zero_wait() {
        // Wait{0} を返すとウォッチャが busy loop になる。全ての Wait は 1ms 以上
        for now in 0..40_000i64 {
            if let SilenceStep::Wait { ms } = silence_step(now, 0, T) {
                assert!(ms >= 1, "now={now} で Wait{{0}} が返った");
            }
        }
    }
}
