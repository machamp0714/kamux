use std::io::Read;
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};

fn relay_bin() -> &'static str {
    env!("CARGO_BIN_EXE_kamux-relay")
}

/// テスト用の使い捨てソケットパス。$TMPDIR 直下に置く。
fn temp_sock(tag: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "kamux-relay-test-{}-{}.sock",
        tag,
        std::process::id()
    ));
    let _ = std::fs::remove_file(&p);
    p
}

#[test]
fn exits_zero_with_no_env_and_no_args() {
    let out = Command::new(relay_bin())
        .stdin(Stdio::null())
        .env_remove("KAMUX_SESSION_ID")
        .env_remove("KAMUX_HOOKS_SOCK")
        .output()
        .expect("relay should be executable");

    assert_eq!(out.status.code(), Some(0), "relay must always exit 0");
    assert!(
        out.stdout.is_empty(),
        "stdout must be empty, got: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        out.stderr.is_empty(),
        "stderr must be empty, got: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn exits_zero_when_socket_is_missing() {
    let out = Command::new(relay_bin())
        .arg("Stop")
        .stdin(Stdio::null())
        .env("KAMUX_SESSION_ID", "3f2a0000-0000-4000-8000-000000009c1e")
        .env(
            "KAMUX_HOOKS_SOCK",
            "/nonexistent/kamux-hooks-does-not-exist.sock",
        )
        .output()
        .expect("relay should be executable");

    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    assert!(out.stderr.is_empty());
}

#[test]
fn exits_zero_when_session_id_is_malformed() {
    let sock = temp_sock("malformed");
    let listener = UnixListener::bind(&sock).expect("bind");

    let out = Command::new(relay_bin())
        .arg("Stop")
        .stdin(Stdio::null())
        .env("KAMUX_SESSION_ID", "not-a-uuid")
        .env("KAMUX_HOOKS_SOCK", &sock)
        .output()
        .expect("relay should be executable");

    assert_eq!(out.status.code(), Some(0));
    listener.set_nonblocking(true).expect("set_nonblocking");
    assert!(
        listener.accept().is_err(),
        "malformed session id must not be sent"
    );
    let _ = std::fs::remove_file(&sock);
}

#[test]
fn forwards_stdin_payload_to_socket() {
    use std::io::Write;

    let sock = temp_sock("forward");
    let listener = UnixListener::bind(&sock).expect("bind");

    let sock_for_child = sock.clone();
    let child = std::thread::spawn(move || {
        let mut proc = Command::new(relay_bin())
            .arg("Notification")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("KAMUX_SESSION_ID", "3f2a0000-0000-4000-8000-000000009c1e")
            .env("KAMUX_HOOKS_SOCK", &sock_for_child)
            .spawn()
            .expect("spawn relay");
        proc.stdin
            .as_mut()
            .expect("stdin")
            .write_all(br#"{"session_id":"550e8400-e29b-41d4-a716-446655440000"}"#)
            .expect("write stdin");
        proc.wait_with_output().expect("wait")
    });

    let (mut stream, _) = listener.accept().expect("accept");
    let mut buf = String::new();
    stream.read_to_string(&mut buf).expect("read to EOF");

    let out = child.join().expect("join");
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    assert!(out.stderr.is_empty());

    let v: serde_json::Value = serde_json::from_str(&buf).expect("valid JSON on the wire");
    assert_eq!(v["v"], 1);
    assert_eq!(
        v["kamux_session_id"],
        "3f2a0000-0000-4000-8000-000000009c1e"
    );
    assert_eq!(v["hook_kind"], "Notification");
    assert_eq!(
        v["payload"]["session_id"],
        "550e8400-e29b-41d4-a716-446655440000"
    );

    let _ = std::fs::remove_file(&sock);
}
