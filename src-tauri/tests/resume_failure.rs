//! 再開失敗の統合検証。
//! fake-agent-resume-fail.sh は「--resume に無効な ID を渡された claude」を模し、
//! SessionStart を発火せずに非ゼロ終了する。その終了コードを ResumeTracker に
//! 流し込み、ResumeFailed に分類されることを確認する。

use std::path::PathBuf;
use std::process::Command;

use kamux::model::StateReason;
use kamux::session::cli_args::ResumePlan;
use kamux::session::resume_tracker::ResumeTracker;

const SID: &str = "11111111-1111-4111-8111-111111111111";

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-agent-resume-fail.sh")
}

#[test]
fn fake_agent_exits_nonzero_without_session_start() {
    let output = Command::new("/bin/sh")
        .arg(fixture_path())
        .env("KAMUX_SESSION_ID", SID)
        .output()
        .expect("fake agent should run");

    assert_eq!(output.status.code(), Some(3), "終了コードが契約と違う");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("No conversation found"),
        "stderr が想定と違う: {stderr}"
    );
}

#[test]
fn tracker_classifies_the_fake_agent_exit_as_resume_failed() {
    let output = Command::new("/bin/sh")
        .arg(fixture_path())
        .env("KAMUX_SESSION_ID", SID)
        .output()
        .expect("fake agent should run");

    // フィクスチャが存在しない/差し替えられている場合、/bin/sh は 127 のような
    // 別の非ゼロ終了コードを返す。それでも classify_exit は「非ゼロなら
    // ResumeFailed」の 1 ビットしか見ないため、下の assert_eq! だけでは
    // フィクスチャ不在を判別できない(このフィクスチャが表す「exit 3」の
    // 主張を独立に固定する)。
    assert_eq!(
        output.status.code(),
        Some(3),
        "終了コードが契約と違う(フィクスチャが想定と別物になっている)"
    );

    // session_id(map のキー)と claude_session_id(ResumePlan の中身)は
    // 同じ String 型で名前も同義に読めるため、意図的に別値にしておく
    // (契約 §81.2 条件 1・2)。同値のままだと、tracker が誤って
    // claude_session_id をキーに使っていても本テストは気づけない。
    let claude_session_id = "550e8400-e29b-41d4-a716-446655440000";
    let tracker = ResumeTracker::new();
    tracker.mark_resume_attempt(
        SID,
        &ResumePlan::ClaudeResume {
            claude_session_id: claude_session_id.to_string(),
        },
    );
    // SessionStart は届かない(fake-agent が発火しない)
    // classify_exit は surface_id を取る(契約 §41.3)
    let reason = tracker.classify_exit(&format!("{SID}:agent"), output.status.code());

    assert_eq!(reason, StateReason::ResumeFailed);
}
