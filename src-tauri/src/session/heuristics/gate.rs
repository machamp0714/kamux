//! ヒューリスティック入力から状態遷移への変換。
//!
//! ここは「優先度比較」ではなく「発生自体の抑止」として実装する。
//! hook 由来の遷移と競争させないため、レースによる不定性が入らない。

use super::hook_liveness::HookLiveness;
use crate::model::RuntimeState;
use crate::session::runtime_state::StateInput;

/// ヒューリスティック検知の入力種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeuristicInput {
    /// 本物の BEL を検知した
    Bel,
    /// 沈黙タイムアウトが成立した
    Silence,
}

/// ゲート判定に必要なセッションの文脈。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeuristicContext {
    pub current: RuntimeState,
    pub heuristics_enabled: bool,
    pub hook_liveness: HookLiveness,
}

/// ヒューリスティック入力を**状態機械への入力**に変換する。抑止する場合は `None`。
///
/// 契約 §41.4: 返すのは `StateInput` であって次の状態ではない。次の状態を決める
/// 権限は M2-1 §2.2 の遷移表だけが持つ。ここが返しうるのは `BelDetected` と
/// `SilenceTimeout` の 2 値のみで、どちらも遷移表上 `waiting_input` / `idle` にしか
/// 到達しない(契約 §2 のとおり `Interrupted` は実行中に付かない)。
///
/// 現在状態による抑止は遷移表と重複するが、ゲートは「送らない」ことしかできないため
/// 不正な状態が書かれることは原理的に起きない(契約 §41.4)。
pub fn heuristic_transition(ctx: HeuristicContext, input: HeuristicInput) -> Option<StateInput> {
    // 1. セッション単位のオフ設定
    if !ctx.heuristics_enabled {
        return None;
    }
    // 2. hook が生きているなら権威ある遷移だけを採用する
    // 3. 猶予中もまだ hook を待つ
    if matches!(
        ctx.hook_liveness,
        HookLiveness::Healthy | HookLiveness::Pending
    ) {
        return None;
    }
    // 4. 死んだ / 中断した / 起動に失敗したセッションに推定を被せない
    if matches!(
        ctx.current,
        RuntimeState::Exited | RuntimeState::Interrupted | RuntimeState::Error
    ) {
        return None;
    }

    match input {
        HeuristicInput::Bel => match ctx.current {
            RuntimeState::Running | RuntimeState::Idle => Some(StateInput::BelDetected),
            // 5/6. すでに入力待ち。同じ通知を繰り返さない
            RuntimeState::WaitingInput => None,
            RuntimeState::Exited | RuntimeState::Interrupted | RuntimeState::Error => None,
        },
        HeuristicInput::Silence => match ctx.current {
            RuntimeState::Running => Some(StateInput::SilenceTimeout),
            // 8. 入力待ちの方が情報量が多い。沈黙は新しい情報を持たない
            RuntimeState::WaitingInput | RuntimeState::Idle => None,
            RuntimeState::Exited | RuntimeState::Interrupted | RuntimeState::Error => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 契約 §2 の 6 値すべて（error を落とすとゲートの網羅テストが穴を持つ）
    const ALL_STATES: [RuntimeState; 6] = [
        RuntimeState::Running,
        RuntimeState::WaitingInput,
        RuntimeState::Idle,
        RuntimeState::Exited,
        RuntimeState::Interrupted,
        RuntimeState::Error,
    ];

    fn ctx(current: RuntimeState) -> HeuristicContext {
        HeuristicContext {
            current,
            heuristics_enabled: true,
            hook_liveness: HookLiveness::NotApplicable,
        }
    }

    #[test]
    fn bel_promotes_running_to_waiting_input() {
        assert_eq!(
            heuristic_transition(ctx(RuntimeState::Running), HeuristicInput::Bel),
            Some(StateInput::BelDetected)
        );
    }

    #[test]
    fn bel_promotes_idle_to_waiting_input() {
        assert_eq!(
            heuristic_transition(ctx(RuntimeState::Idle), HeuristicInput::Bel),
            Some(StateInput::BelDetected)
        );
    }

    #[test]
    fn bel_on_waiting_input_is_suppressed() {
        // 同じ通知を繰り返さない
        assert_eq!(
            heuristic_transition(ctx(RuntimeState::WaitingInput), HeuristicInput::Bel),
            None
        );
    }

    #[test]
    fn silence_moves_running_to_idle() {
        assert_eq!(
            heuristic_transition(ctx(RuntimeState::Running), HeuristicInput::Silence),
            Some(StateInput::SilenceTimeout)
        );
    }

    #[test]
    fn silence_does_not_overwrite_waiting_input() {
        // waiting_input の方が情報量が多い（設計 §4.8 ルール 8）
        assert_eq!(
            heuristic_transition(ctx(RuntimeState::WaitingInput), HeuristicInput::Silence),
            None
        );
    }

    #[test]
    fn silence_on_idle_is_suppressed() {
        assert_eq!(
            heuristic_transition(ctx(RuntimeState::Idle), HeuristicInput::Silence),
            None
        );
    }

    #[test]
    fn dead_sessions_never_receive_heuristic_states() {
        for input in [HeuristicInput::Bel, HeuristicInput::Silence] {
            for state in [RuntimeState::Exited, RuntimeState::Interrupted] {
                assert_eq!(
                    heuristic_transition(ctx(state), input),
                    None,
                    "{state:?} / {input:?}"
                );
            }
        }
    }

    #[test]
    fn disabled_sessions_suppress_everything() {
        for input in [HeuristicInput::Bel, HeuristicInput::Silence] {
            for state in ALL_STATES {
                let c = HeuristicContext {
                    heuristics_enabled: false,
                    ..ctx(state)
                };
                assert_eq!(
                    heuristic_transition(c, input),
                    None,
                    "{state:?} / {input:?}"
                );
            }
        }
    }

    #[test]
    fn healthy_hooks_suppress_everything() {
        // hook 由来の遷移が常に勝つ（設計 §4.8 ルール 2）
        for input in [HeuristicInput::Bel, HeuristicInput::Silence] {
            for state in ALL_STATES {
                let c = HeuristicContext {
                    hook_liveness: HookLiveness::Healthy,
                    ..ctx(state)
                };
                assert_eq!(
                    heuristic_transition(c, input),
                    None,
                    "{state:?} / {input:?}"
                );
            }
        }
    }

    #[test]
    fn pending_hooks_suppress_everything() {
        for input in [HeuristicInput::Bel, HeuristicInput::Silence] {
            for state in ALL_STATES {
                let c = HeuristicContext {
                    hook_liveness: HookLiveness::Pending,
                    ..ctx(state)
                };
                assert_eq!(
                    heuristic_transition(c, input),
                    None,
                    "{state:?} / {input:?}"
                );
            }
        }
    }

    #[test]
    fn unreachable_hooks_allow_the_fallback() {
        // 設計書 §12「hooks 不達 → 汎用ヒューリスティックへ自動フォールバック」
        let c = HeuristicContext {
            hook_liveness: HookLiveness::Unreachable,
            ..ctx(RuntimeState::Running)
        };
        assert_eq!(
            heuristic_transition(c, HeuristicInput::Silence),
            Some(StateInput::SilenceTimeout)
        );
    }

    /// 契約 §41.4: ゲートが出せる入力は推定由来の 2 値だけである。
    /// `Spawned` / `PtyExited` / `HookStop` などの権威ある入力を名乗らせない。
    #[test]
    fn heuristics_only_emit_estimated_inputs() {
        for input in [HeuristicInput::Bel, HeuristicInput::Silence] {
            for state in ALL_STATES {
                for liveness in [HookLiveness::NotApplicable, HookLiveness::Unreachable] {
                    let c = HeuristicContext {
                        hook_liveness: liveness,
                        ..ctx(state)
                    };
                    if let Some(emitted) = heuristic_transition(c, input) {
                        assert!(
                            matches!(
                                emitted,
                                StateInput::BelDetected | StateInput::SilenceTimeout
                            ),
                            "権威ある入力 {emitted:?} をヒューリスティックが名乗った"
                        );
                    }
                }
            }
        }
    }

    /// 契約 §41.4: ゲートの抑止は M2-1 §2.2 の遷移表の部分集合でなければならない。
    /// ゲートが通した入力が遷移表で禁止されているなら、どちらかがドリフトしている。
    #[test]
    fn gate_never_emits_an_input_the_transition_table_forbids() {
        use crate::session::runtime_state::next_state;
        for input in [HeuristicInput::Bel, HeuristicInput::Silence] {
            for state in ALL_STATES {
                if let Some(emitted) = heuristic_transition(ctx(state), input) {
                    assert!(
                        next_state(state, emitted).is_some(),
                        "{state:?} × {emitted:?} は遷移表で禁止されている"
                    );
                }
            }
        }
    }
}
