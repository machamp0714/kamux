use std::io::Read;
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn relay_bin() -> &'static str {
    env!("CARGO_BIN_EXE_kamux-relay")
}

/// テスト用の使い捨てソケットパス。$TMPDIR 直下に置く。
/// macOS の `sun_path` は 104 バイト上限なので tag は短く保つ。
fn temp_sock(tag: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "kamux-relay-test-{}-{}.sock",
        tag,
        std::process::id()
    ));
    let _ = std::fs::remove_file(&p);
    p
}

/// `forwards_stdin_payload_to_socket` 等が relay からの接続を待つ上限。
/// 正常系は 1 秒未満で終わるが、CI の遅さを見込んで 10 秒を確保する。
const SOCKET_WAIT: Duration = Duration::from_secs(10);

fn accept_one_message(listener: &UnixListener) -> std::io::Result<String> {
    let (mut stream, _) = listener.accept()?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf)?;
    Ok(buf)
}

/// `listener.accept()` と後続の `read_to_string` の両方を `SOCKET_WAIT` で有界化する。
///
/// `UnixListener` には accept にタイムアウトを掛ける API が無いため、別スレッドで
/// blocking の accept + read を行わせ、呼び出し側は `mpsc::recv_timeout` で待つ。
/// この 1 本の bound が accept 段・read 段の両方を覆う —— send は read が完了して
/// 初めて呼ばれるので、どちらで止まっても同じ経路でタイムアウトする。relay が
/// 誤って接続しない・接続後に何も送らない、という変異を有限時間で検出できる。
fn accept_and_read_with_timeout(listener: UnixListener) -> String {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = accept_one_message(&listener);
        let _ = tx.send(result);
    });

    match rx.recv_timeout(SOCKET_WAIT) {
        Ok(Ok(buf)) => buf,
        Ok(Err(e)) => panic!("listener.accept() または read_to_string が失敗した: {e}"),
        Err(_) => panic!(
            "listener.accept() + read_to_string が {SOCKET_WAIT:?} 以内に完了しなかった \
             (relay がソケットへ接続していないか、接続後に何も送っていない可能性がある)"
        ),
    }
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

    let buf = accept_and_read_with_timeout(listener);

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

#[test]
fn exits_zero_when_hook_kind_is_empty() {
    let sock = temp_sock("hke");
    let listener = UnixListener::bind(&sock).expect("bind");

    let out = Command::new(relay_bin())
        .arg("")
        .stdin(Stdio::null())
        .env("KAMUX_SESSION_ID", "3f2a0000-0000-4000-8000-000000009c1e")
        .env("KAMUX_HOOKS_SOCK", &sock)
        .output()
        .expect("relay should be executable");

    assert_eq!(out.status.code(), Some(0));
    listener.set_nonblocking(true).expect("set_nonblocking");
    assert!(
        listener.accept().is_err(),
        "empty hook kind must not be sent"
    );
    let _ = std::fs::remove_file(&sock);
}

#[test]
fn exits_zero_when_hook_kind_has_non_alphanumeric_characters() {
    let sock = temp_sock("hks");
    let listener = UnixListener::bind(&sock).expect("bind");

    let out = Command::new(relay_bin())
        .arg("Stop!")
        .stdin(Stdio::null())
        .env("KAMUX_SESSION_ID", "3f2a0000-0000-4000-8000-000000009c1e")
        .env("KAMUX_HOOKS_SOCK", &sock)
        .output()
        .expect("relay should be executable");

    assert_eq!(out.status.code(), Some(0));
    listener.set_nonblocking(true).expect("set_nonblocking");
    assert!(
        listener.accept().is_err(),
        "hook kind with symbols must not be sent"
    );
    let _ = std::fs::remove_file(&sock);
}

#[test]
fn exits_zero_when_hook_kind_exceeds_max_length() {
    let sock = temp_sock("hkl");
    let listener = UnixListener::bind(&sock).expect("bind");
    let too_long = "A".repeat(33);

    let out = Command::new(relay_bin())
        .arg(&too_long)
        .stdin(Stdio::null())
        .env("KAMUX_SESSION_ID", "3f2a0000-0000-4000-8000-000000009c1e")
        .env("KAMUX_HOOKS_SOCK", &sock)
        .output()
        .expect("relay should be executable");

    assert_eq!(out.status.code(), Some(0));
    listener.set_nonblocking(true).expect("set_nonblocking");
    assert!(
        listener.accept().is_err(),
        "hook kind longer than 32 chars must not be sent"
    );
    let _ = std::fs::remove_file(&sock);
}

#[test]
fn flags_truncated_when_stdin_exceeds_max_bytes() {
    use std::io::Write;

    // relay 側の MAX_STDIN_BYTES（1 MiB, crates/kamux-relay/src/main.rs）に
    // 確実に当てるため、1 バイト超過させる。
    const OVER_MAX_STDIN_BYTES: usize = 1024 * 1024 + 1;

    let sock = temp_sock("big");
    let listener = UnixListener::bind(&sock).expect("bind");

    let sock_for_child = sock.clone();
    let child = std::thread::spawn(move || {
        let mut proc = Command::new(relay_bin())
            .arg("Stop")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("KAMUX_SESSION_ID", "3f2a0000-0000-4000-8000-000000009c1e")
            .env("KAMUX_HOOKS_SOCK", &sock_for_child)
            .spawn()
            .expect("spawn relay");
        let oversized = vec![b'x'; OVER_MAX_STDIN_BYTES];
        // relay は MAX_STDIN_BYTES ちょうどで stdin を読むのを止めて先に進むため、
        // 末尾の超過分の書き込みが EPIPE になるのは正常系でも起こりうる。
        // このテストが主張したいのはワイヤ上の `truncated` であって write の成否
        // ではないので、書き込み自体の失敗ではテストを落とさない。
        let _ = proc.stdin.as_mut().expect("stdin").write_all(&oversized);
        proc.wait_with_output().expect("wait")
    });

    let buf = accept_and_read_with_timeout(listener);

    let out = child.join().expect("join");
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    assert!(out.stderr.is_empty());

    let v: serde_json::Value = serde_json::from_str(&buf).expect("valid JSON on the wire");
    assert_eq!(
        v["truncated"], true,
        "stdin over MAX_STDIN_BYTES must be flagged as truncated"
    );

    let _ = std::fs::remove_file(&sock);
}
