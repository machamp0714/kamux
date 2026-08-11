//! fake-agent を実プロセスとして走らせ、relay → ソケット → HookSink の全経路を検証する。
//! 契約 §14: 実 claude はテストに使わない。

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

use super::tests::RecordingSink;
use super::{HookKind, HooksServer};

/// テストバイナリは target/{profile}/deps/ に置かれるので、2 つ上が profile ディレクトリ。
fn relay_bin() -> PathBuf {
    let exe = std::env::current_exe().expect("current_exe");
    let profile_dir = exe
        .parent()
        .and_then(|p| p.parent())
        .expect("test binary must live under target/{profile}/deps");
    let relay = profile_dir.join("kamux-relay");
    assert!(
        relay.is_file(),
        "kamux-relay not built at {}. Run `cargo build --workspace` first.",
        relay.display()
    );
    relay
}

fn fake_agent() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-agent.sh")
}

fn wait_for<F: Fn() -> bool>(cond: F) {
    for _ in 0..300 {
        if cond() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("condition not met within 3s");
}

#[test]
fn fake_agent_drives_the_full_hook_sequence() {
    let sock = std::env::temp_dir().join(format!("kamux-e2e-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);

    let sink = Arc::new(RecordingSink::default());
    let server = HooksServer::start(sock.clone(), sink.clone()).expect("start server");

    let kamux_session_id = "3f2a0000-0000-4000-8000-000000009c1e";
    let mut child = Command::new("/bin/sh")
        .arg(fake_agent())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("KAMUX_SESSION_ID", kamux_session_id)
        .env("KAMUX_HOOKS_SOCK", &sock)
        .env("KAMUX_RELAY_BIN", relay_bin())
        .spawn()
        .expect("spawn fake-agent");

    // SessionStart / Notification / PermissionRequest が届くまで待つ = 入力待ちになった
    wait_for(|| sink.snapshot().len() == 3);

    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"hello\n")
        .expect("write stdin");

    let output = child.wait_with_output().expect("wait");
    assert_eq!(output.status.code(), Some(0), "fake-agent must exit 0");

    wait_for(|| sink.snapshot().len() == 4);
    let events = sink.snapshot();

    assert_eq!(events[0].kind, HookKind::SessionStart);
    assert_eq!(events[0].kamux_session_id, kamux_session_id);
    assert_eq!(events[0].claude_session_id.as_deref(), Some("fake-cc-0001"));
    assert_eq!(events[0].source.as_deref(), Some("startup"));

    assert_eq!(events[1].kind, HookKind::Notification);
    assert_eq!(events[1].kamux_session_id, kamux_session_id);

    // payload が {} でも argv 由来で種別が決まる（設計 §6-2）
    assert_eq!(events[2].kind, HookKind::PermissionRequest);
    assert_eq!(events[2].kamux_session_id, kamux_session_id);
    assert_eq!(events[2].claude_session_id, None);

    assert_eq!(events[3].kind, HookKind::Stop);
    assert_eq!(events[3].claude_session_id.as_deref(), Some("fake-cc-0001"));

    // relay の出力が agent の stdout を汚していないこと（契約 §12.7-2 の前提）
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        vec![
            "fake-agent starting",
            "line 1",
            "line 2",
            "line 3",
            "got input"
        ],
        "relay must not write anything to the agent's stdout"
    );
    assert!(
        output.stderr.is_empty(),
        "relay must not write to stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    drop(server);
    let _ = std::fs::remove_file(&sock);
}

/// アプリが落ちている状況（ソケット無し）でも fake-agent は正常終了する。
#[test]
fn fake_agent_survives_a_dead_socket() {
    let mut child = Command::new("/bin/sh")
        .arg(fake_agent())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("KAMUX_SESSION_ID", "3f2a0000-0000-4000-8000-000000009c1e")
        .env("KAMUX_HOOKS_SOCK", "/nonexistent/kamux-hooks-dead.sock")
        .env("KAMUX_RELAY_BIN", relay_bin())
        .spawn()
        .expect("spawn fake-agent");

    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"hello\n")
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");

    assert_eq!(
        output.status.code(),
        Some(0),
        "a dead socket must never break the agent"
    );
    assert!(output.stderr.is_empty());
}
