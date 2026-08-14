use std::collections::HashMap;
use std::sync::Mutex;

use crate::model::StateReason;
use crate::pty::agent_session_id;

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
    /// **呼び出し側は、`resume_mode()` が `ResumeMode::None` を返す入力
    /// (第1部 §3 行 4〜16。shell / custom / codex の全行、および
    /// claude + in_place + `claude_session_id == None`)で、この試行を
    /// 記録するかどうかを決めること。**
    /// 記録すると、`SessionStart` hook を発火しない `cli_kind`(shell /
    /// custom / codex)では `session_start_seen` が永久に `false` のまま
    /// になるため、そのプロセスが非ゼロ終了しさえすれば必ず
    /// `StateReason::ResumeFailed` になる。これは「会話復元を試みてすら
    /// いない」プロセスに対して誤って再開失敗を報告することになる。
    ///
    /// 未検証の予測(この呼び出し側は本 PR にはまだ無く、変異を当てる
    /// 対象そのものが存在しない): どの手当てを採るかは Task 8 の設計
    /// 判断であり、選択肢は少なくとも 3 つある。
    ///   (a) `resume_mode(&plan) != ResumeMode::None` のときだけ呼ぶ
    ///   (b) 呼ぶが、`ResumeFailed` の判定に「そもそも復元を試みた」
    ///       条件を持たせる
    ///   (c) 非 claude では resume ボタン自体を「再起動」として
    ///       `ResumeFailed` 経路から外す
    pub fn mark_resume_attempt(&self, session_id: &str) {
        let mut map = self.lock();
        map.insert(
            session_id.to_string(),
            Attempt {
                session_start_seen: false,
            },
        );
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

    // mark_resume_attempt / note_session_start は session_id、
    // classify_exit は surface_id を取る(契約 §41.3 決定 (3))。
    const S: &str = "11111111-1111-4111-8111-111111111111";
    const SURF: &str = "11111111-1111-4111-8111-111111111111:agent";

    #[test]
    fn plain_start_exit_is_pty_exited() {
        let t = ResumeTracker::new();
        assert_eq!(t.classify_exit(SURF, Some(1)), StateReason::PtyExited);
    }

    /// エディタサーフェスの終了は分類対象外で、試行フラグを消費しない(契約 §2 / §41.3)。
    #[test]
    fn editor_surface_exit_never_consumes_the_attempt() {
        let t = ResumeTracker::new();
        t.mark_resume_attempt(S);
        let editor = "11111111-1111-4111-8111-111111111111:editor";
        assert_eq!(t.classify_exit(editor, Some(1)), StateReason::PtyExited);
        // 試行は生きているので、agent の終了は依然 ResumeFailed になる。
        assert_eq!(t.classify_exit(SURF, Some(1)), StateReason::ResumeFailed);
    }

    /// 第1部 §4.2: 3 条件の論理積が成立したときだけ ResumeFailed。
    #[test]
    fn resume_attempt_without_session_start_and_nonzero_exit_is_resume_failed() {
        let t = ResumeTracker::new();
        t.mark_resume_attempt(S);
        assert_eq!(t.classify_exit(SURF, Some(1)), StateReason::ResumeFailed);
    }

    /// SessionStart が届いていれば、その resume は成功している。
    #[test]
    fn session_start_received_means_resume_succeeded() {
        let t = ResumeTracker::new();
        t.mark_resume_attempt(S);
        t.note_session_start(S);
        assert_eq!(t.classify_exit(SURF, Some(1)), StateReason::PtyExited);
    }

    /// 誤検知防止: 正常に再開して即座に終了したケース。
    #[test]
    fn zero_exit_code_is_never_resume_failed() {
        let t = ResumeTracker::new();
        t.mark_resume_attempt(S);
        assert_eq!(t.classify_exit(SURF, Some(0)), StateReason::PtyExited);
    }

    /// シグナル終了などで exit_code が取れない場合も失敗断定しない。
    #[test]
    fn unknown_exit_code_is_never_resume_failed() {
        let t = ResumeTracker::new();
        t.mark_resume_attempt(S);
        assert_eq!(t.classify_exit(SURF, None), StateReason::PtyExited);
    }

    /// 分類したら試行は消費される。次の起動に持ち越さない。
    #[test]
    fn classify_consumes_the_attempt() {
        let t = ResumeTracker::new();
        t.mark_resume_attempt(S);
        assert_eq!(t.classify_exit(SURF, Some(1)), StateReason::ResumeFailed);
        assert_eq!(t.classify_exit(SURF, Some(1)), StateReason::PtyExited);
    }

    /// 再試行のたびに状態はリセットされる。
    #[test]
    fn a_new_attempt_resets_the_session_start_flag() {
        let t = ResumeTracker::new();
        t.mark_resume_attempt(S);
        t.note_session_start(S);
        t.mark_resume_attempt(S);
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

    #[test]
    fn sessions_are_tracked_independently() {
        let other = "22222222-2222-4222-8222-222222222222";
        let other_surf = format!("{other}:agent");
        let t = ResumeTracker::new();
        t.mark_resume_attempt(S);
        t.note_session_start(other);
        assert_eq!(t.classify_exit(SURF, Some(1)), StateReason::ResumeFailed);
        assert_eq!(
            t.classify_exit(&other_surf, Some(1)),
            StateReason::PtyExited
        );
    }
}
