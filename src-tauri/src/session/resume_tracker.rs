use std::collections::HashMap;
use std::sync::Mutex;

use crate::model::StateReason;
use crate::pty::agent_session_id;
use crate::session::cli_args::ResumePlan;

/// resume 試行 1 回分の観測。
#[derive(Debug, Clone, Copy)]
struct Attempt {
    session_start_seen: bool,
}

/// 再開が失敗したかを判定する(第1部 §4.2)。
///
/// 判定条件は 3 つの論理積:
///   1. resume 試行中フラグが立っている
///   2. その試行以降 SessionStart hook を受信していない
///   3. PTY が非ゼロ終了した
///
/// 2 が本命の材料。SessionStart まで到達していれば resume は成功している。
/// 時間窓だけで判定すると、正常に再開したセッションをユーザーが即座に
/// 終了した場合に誤検知し、有効な claude_session_id を捨ててしまう。
#[derive(Debug, Default)]
pub struct ResumeTracker {
    attempts: Mutex<HashMap<String, Attempt>>,
}

impl ResumeTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// resume_session が PTY を spawn する直前に呼ぶ。
    /// 同一セッションの前回試行は破棄される。
    ///
    /// **`ResumePlan::FreshStart` は試行として記録しない**(Task 8 が採った形。
    /// 前身の doc が並べていた 3 つの選択肢のうち (a) に当たる)。理由:
    /// `SpawnIntent::Resume` は「会話復元を試みている」と同値ではない ——
    /// shell / custom / codex の全行と claude + in_place + `claude_session_id`
    /// 欠損では `resume_plan()` が `FreshStart` を返し、復元は試みられない。
    /// これらの `cli_kind` は `SessionStart` hook を発火しない(発火源は
    /// `claude_hook_args` の `--settings` で `CliKind::Claude` 限定)ので、
    /// 無条件に記録すると `session_start_seen` が永久に `false` のままになり、
    /// そのプロセスが非ゼロ終了しさえすれば必ず `StateReason::ResumeFailed` に
    /// なる。「会話は復元されませんでした」が、復元を試みてすらいない
    /// プロセスに対して出ることになる。
    ///
    /// **ガードは呼び出し側ではなくここに置く。** `resume_session` は
    /// `state.pty.spawn`(Wry 固定。契約 §15)を含みユニットテストから到達
    /// できない(契約 §96.4 の到達不能領域)ので、あちらに `if` を書くと
    /// ガードを外す変異が全緑になる。ここに置けば観測できる —— このガードを
    /// 外して無条件に記録する変異は `a_fresh_start_plan_is_never_recorded_as_a_resume_attempt`
    /// と `a_fresh_start_for_an_ambiguous_in_place_conversation_is_not_recorded_either`
    /// を赤にすることを実測した(Task 8 の変異 M-1)。
    pub fn mark_resume_attempt(&self, session_id: &str, plan: &ResumePlan) {
        // 理由(`FreshStartReason`)では分岐しない —— `resume_mode()` が理由に
        // よらず `ResumeMode::None` へ落とすのと同じ扱いにする。
        if matches!(plan, ResumePlan::FreshStart { .. }) {
            return;
        }
        let mut map = self.lock();
        map.insert(
            session_id.to_string(),
            Attempt {
                session_start_seen: false,
            },
        );
    }

    /// 記録した試行を破棄する。**PTY が上がらなかったときに呼ぶ**（`resume_session`
    /// の spawn 失敗腕）。冪等 —— 未知のセッションでは何もしない。
    ///
    /// **これが無いと `mark_resume_attempt` の記録が漏れ出す。** 記録は spawn の
    /// **直前**（契約 §123.6 の 5）なので、spawn が `Err` を返した終了は
    /// `PtySink::on_exit` を一度も通らず、`classify_exit` による消費が起こらない。
    /// 残った試行は次にそのセッションで起きた非ゼロ終了 —— たとえば
    /// `start_session`（会話復元を試みない起動）—— を `ResumeFailed` に化けさせる。
    /// `mark_resume_attempt` の `FreshStart` ガードが閉じたのと同じ害
    /// （復元を試みてすらいない起動に「会話は復元されませんでした」が出る）が、
    /// 別経路で開いたままになる。
    ///
    /// **判断をここへ置く理由も `mark_resume_attempt` と同じである。** 呼び出し側の
    /// `resume_session` は `state.pty.spawn_with_observer`（Wry 固定。契約 §15）を
    /// 含みユニットテストから到達できない（契約 §96.4）ため、あちらに条件を書くと
    /// 外す変異が緑になる。ここなら観測できる —— **本体を no-op（`{}`）にする変異は
    /// `a_cleared_attempt_is_no_longer_classified_as_a_resume_failure` を赤にする
    /// ことを実測した**（Task 8 の変異 M-11）。
    pub fn clear_resume_attempt(&self, session_id: &str) {
        self.lock().remove(session_id);
    }

    /// SessionStart hook 受信時に呼ぶ(source の値によらず)。
    /// 試行が記録されていないセッションでは何もしない。
    pub fn note_session_start(&self, session_id: &str) {
        let mut map = self.lock();
        if let Some(attempt) = map.get_mut(session_id) {
            attempt.session_start_seen = true;
        }
    }

    /// PTY 終了時に呼ぶ。試行は消費され、次の起動には持ち越さない。
    /// 引数は `surface_id`(`PtySink::on_exit` が持つのはこれだけ。契約 §41.3)。
    /// `:agent` 以外は常に `PtyExited` を返し、試行フラグを消費しない ——
    /// nvim を閉じただけで再開失敗の判定材料が失われてはならない。
    /// 極性判定は `crate::pty::agent_session_id` に委ねる(契約 §41.3 決定 (3) /
    /// lane-controller 裁定 16。手書きの `strip_suffix(":agent")` は、
    /// `surface_id` の逆関数をここへもう 1 つ増やすことになり、片方だけ直した
    /// 瞬間に「nvim を閉じただけでセッション側の何かが死ぬ」が戻ってくる)。
    pub fn classify_exit(&self, surface_id: &str, exit_code: Option<i32>) -> StateReason {
        let Some(session_id) = agent_session_id(surface_id) else {
            return StateReason::PtyExited;
        };
        let mut map = self.lock();
        let attempt = match map.remove(session_id) {
            Some(a) => a,
            None => return StateReason::PtyExited,
        };
        let failed_hard = matches!(exit_code, Some(code) if code != 0);
        if !attempt.session_start_seen && failed_hard {
            StateReason::ResumeFailed
        } else {
            StateReason::PtyExited
        }
    }

    /// Mutex が毒された場合も内部状態を捨てて続行する(契約 §0: panic 経路を作らない)。
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Attempt>> {
        match self.attempts.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::StateReason;

    use crate::session::cli_args::FreshStartReason;

    // mark_resume_attempt / note_session_start は session_id、
    // classify_exit は surface_id を取る(契約 §41.3 決定 (3))。
    const S: &str = "11111111-1111-4111-8111-111111111111";
    const SURF: &str = "11111111-1111-4111-8111-111111111111:agent";

    /// 会話復元を実際に試みる plan。`mark_resume_attempt` が試行を記録する側。
    const ATTEMPTED: ResumePlan = ResumePlan::ClaudeContinue;

    #[test]
    fn plain_start_exit_is_pty_exited() {
        let t = ResumeTracker::new();
        assert_eq!(t.classify_exit(SURF, Some(1)), StateReason::PtyExited);
    }

    /// エディタサーフェスの終了は分類対象外で、試行フラグを消費しない(契約 §2 / §41.3)。
    #[test]
    fn editor_surface_exit_never_consumes_the_attempt() {
        let t = ResumeTracker::new();
        t.mark_resume_attempt(S, &ATTEMPTED);
        let editor = "11111111-1111-4111-8111-111111111111:editor";
        assert_eq!(t.classify_exit(editor, Some(1)), StateReason::PtyExited);
        // 試行は生きているので、agent の終了は依然 ResumeFailed になる。
        assert_eq!(t.classify_exit(SURF, Some(1)), StateReason::ResumeFailed);
    }

    /// 第1部 §4.2: 3 条件の論理積が成立したときだけ ResumeFailed。
    #[test]
    fn resume_attempt_without_session_start_and_nonzero_exit_is_resume_failed() {
        let t = ResumeTracker::new();
        t.mark_resume_attempt(S, &ATTEMPTED);
        assert_eq!(t.classify_exit(SURF, Some(1)), StateReason::ResumeFailed);
    }

    /// SessionStart が届いていれば、その resume は成功している。
    #[test]
    fn session_start_received_means_resume_succeeded() {
        let t = ResumeTracker::new();
        t.mark_resume_attempt(S, &ATTEMPTED);
        t.note_session_start(S);
        assert_eq!(t.classify_exit(SURF, Some(1)), StateReason::PtyExited);
    }

    /// 誤検知防止: 正常に再開して即座に終了したケース。
    #[test]
    fn zero_exit_code_is_never_resume_failed() {
        let t = ResumeTracker::new();
        t.mark_resume_attempt(S, &ATTEMPTED);
        assert_eq!(t.classify_exit(SURF, Some(0)), StateReason::PtyExited);
    }

    /// シグナル終了などで exit_code が取れない場合も失敗断定しない。
    #[test]
    fn unknown_exit_code_is_never_resume_failed() {
        let t = ResumeTracker::new();
        t.mark_resume_attempt(S, &ATTEMPTED);
        assert_eq!(t.classify_exit(SURF, None), StateReason::PtyExited);
    }

    /// 分類したら試行は消費される。次の起動に持ち越さない。
    #[test]
    fn classify_consumes_the_attempt() {
        let t = ResumeTracker::new();
        t.mark_resume_attempt(S, &ATTEMPTED);
        assert_eq!(t.classify_exit(SURF, Some(1)), StateReason::ResumeFailed);
        assert_eq!(t.classify_exit(SURF, Some(1)), StateReason::PtyExited);
    }

    /// 再試行のたびに状態はリセットされる。
    #[test]
    fn a_new_attempt_resets_the_session_start_flag() {
        let t = ResumeTracker::new();
        t.mark_resume_attempt(S, &ATTEMPTED);
        t.note_session_start(S);
        t.mark_resume_attempt(S, &ATTEMPTED);
        assert_eq!(t.classify_exit(SURF, Some(1)), StateReason::ResumeFailed);
    }

    /// `note_session_start` は、試行が記録されていないセッションでは
    /// `attempts` に新規エントリを作らない(不変条件。契約 §41.3)。
    /// この不変条件は `classify_exit` の戻り値からは観測できない
    /// (`session_start_seen == true` は常に `PtyExited` へ収束するため)。
    #[test]
    fn note_session_start_does_not_create_an_entry_for_an_unknown_session() {
        let t = ResumeTracker::new();
        t.note_session_start(S);
        assert_eq!(t.lock().len(), 0);
    }

    /// `ResumePlan::FreshStart` は「そもそも会話復元を試みない」決定なので、
    /// 試行として記録しない。記録すると、`SessionStart` hook を発火しない
    /// cli_kind(shell / custom / codex)のプロセスが非ゼロ終了しさえすれば
    /// 必ず `ResumeFailed` になり、復元を試みてすらいない起動に対して
    /// 「会話は復元されませんでした」が出る。
    ///
    /// 陽性の対照は上の
    /// `resume_attempt_without_session_start_and_nonzero_exit_is_resume_failed`
    /// が持つ ——「同じ引数で `ATTEMPTED` なら `ResumeFailed`」なので、
    /// この assert が緑なのは分岐が効いているからであって、
    /// `classify_exit` が常に `PtyExited` を返すからではない。
    #[test]
    fn a_fresh_start_plan_is_never_recorded_as_a_resume_attempt() {
        let t = ResumeTracker::new();
        t.mark_resume_attempt(
            S,
            &ResumePlan::FreshStart {
                reason: FreshStartReason::NoConversationRestore,
            },
        );
        assert_eq!(t.classify_exit(SURF, Some(1)), StateReason::PtyExited);
    }

    /// 同じガードを、もう一方の `FreshStartReason` でも見る。理由で分岐して
    /// いないこと(`resume_mode()` が理由によらず `ResumeMode::None` へ落とすのと
    /// 同じ扱い)の固定。
    #[test]
    fn a_fresh_start_for_an_ambiguous_in_place_conversation_is_not_recorded_either() {
        let t = ResumeTracker::new();
        t.mark_resume_attempt(
            S,
            &ResumePlan::FreshStart {
                reason: FreshStartReason::AmbiguousInPlaceConversation,
            },
        );
        assert_eq!(t.classify_exit(SURF, Some(1)), StateReason::PtyExited);
    }

    /// `ClaudeResume`(分岐表 行 1 / 行 2)も記録される側であることを固定する。
    /// ガードを `ClaudeContinue` だけに絞る変異を弁別する。
    #[test]
    fn a_claude_resume_plan_is_recorded_as_a_resume_attempt() {
        let t = ResumeTracker::new();
        t.mark_resume_attempt(
            S,
            &ResumePlan::ClaudeResume {
                claude_session_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            },
        );
        assert_eq!(t.classify_exit(SURF, Some(1)), StateReason::ResumeFailed);
    }

    /// spawn が失敗した試行は破棄され、次の起動へ持ち越さない。
    ///
    /// 破棄しないと、`start_session`(会話復元を試みない起動)の PTY が非ゼロ終了
    /// しただけで `ResumeFailed` になる —— `mark_resume_attempt` の `FreshStart`
    /// ガードが閉じた害と同型のものが、spawn 失敗という別経路で開く。
    ///
    /// 陽性の対照は
    /// `resume_attempt_without_session_start_and_nonzero_exit_is_resume_failed`
    /// が持つ ——**破棄しない同じ入力は `ResumeFailed` になる。** 片側だけだと、
    /// `classify_exit` が常に `PtyExited` を返しているのか、破棄が効いているのかを
    /// 区別できない。
    #[test]
    fn a_cleared_attempt_is_no_longer_classified_as_a_resume_failure() {
        let t = ResumeTracker::new();
        t.mark_resume_attempt(S, &ATTEMPTED);
        t.clear_resume_attempt(S);
        assert_eq!(t.classify_exit(SURF, Some(1)), StateReason::PtyExited);
    }

    /// 破棄は**そのセッションだけ**に効く。未知 ID では幻の行も作らない(冪等)。
    ///
    /// **前身の `clearing_an_unknown_session_is_a_no_op` は恒真だった**(レビュー
    /// Minor 2) —— 未知 ID しか渡していなかったため、本体が no-op の実装と
    /// 区別できないのに名前が「no-op であることを見た」と読めた。両方向へ
    /// 弁別力を持たせる:
    ///   - 本体を no-op にすると `S` が残り、`len() == 1` の assert が赤になる
    ///   - 本体を `map.clear()`(全消し)にすると `other` まで消え、赤になる
    ///     (Task 8 の変異 F-2 で実測。**どの assert が先に落ちるかは
    ///     報告の F-2 の行を参照** —— ここには「測った結果」だけを書く)
    #[test]
    fn clearing_one_session_removes_only_that_sessions_attempt() {
        let t = ResumeTracker::new();
        let other = "22222222-2222-4222-8222-222222222222";
        t.mark_resume_attempt(S, &ATTEMPTED);
        t.mark_resume_attempt(other, &ATTEMPTED);

        // 未知 ID の破棄は既存の試行を 1 件も壊さず、幻の行も作らない。
        t.clear_resume_attempt("00000000-0000-4000-8000-000000000000");
        assert_eq!(
            t.lock().len(),
            2,
            "未知 ID の破棄が既存の試行を壊した(または幻の行を作った)"
        );

        t.clear_resume_attempt(S);
        assert_eq!(t.lock().len(), 1, "S の試行が破棄されていない");
        assert_eq!(
            t.classify_exit(&format!("{other}:agent"), Some(1)),
            StateReason::ResumeFailed,
            "他セッションの試行まで巻き込んで破棄している"
        );
    }

    #[test]
    fn sessions_are_tracked_independently() {
        let other = "22222222-2222-4222-8222-222222222222";
        let other_surf = format!("{other}:agent");
        let t = ResumeTracker::new();
        t.mark_resume_attempt(S, &ATTEMPTED);
        t.note_session_start(other);
        assert_eq!(t.classify_exit(SURF, Some(1)), StateReason::ResumeFailed);
        assert_eq!(
            t.classify_exit(&other_surf, Some(1)),
            StateReason::PtyExited
        );
    }
}
