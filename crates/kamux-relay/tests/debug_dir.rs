use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn relay_bin() -> &'static str {
    env!("CARGO_BIN_EXE_kamux-relay")
}

/// `debug_write_failure_does_not_block_socket_forward` が relay からの接続を待つ上限。
const SOCKET_WAIT: Duration = Duration::from_secs(10);

fn accept_one_message(listener: &UnixListener) -> std::io::Result<String> {
    let (mut stream, _) = listener.accept()?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf)?;
    Ok(buf)
}

fn accept_and_read_with_timeout(listener: UnixListener) -> String {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = accept_one_message(&listener);
        let _ = tx.send(result);
    });

    match rx.recv_timeout(SOCKET_WAIT) {
        Ok(Ok(buf)) => buf,
        Ok(Err(e)) => panic!("listener.accept() または read_to_string が失敗した: {e}"),
        Err(_) => {
            panic!("listener.accept() + read_to_string が {SOCKET_WAIT:?} 以内に完了しなかった")
        }
    }
}

#[test]
fn writes_raw_stdin_to_debug_dir() {
    let dir = std::env::temp_dir().join(format!("kamux-relay-debug-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let mut proc = Command::new(relay_bin())
        .arg("Notification")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("KAMUX_SESSION_ID", "3f2a0000-0000-4000-8000-000000009c1e")
        .env("KAMUX_HOOKS_SOCK", "/nonexistent/kamux-hooks-none.sock")
        .env("KAMUX_RELAY_DEBUG_DIR", &dir)
        .spawn()
        .expect("spawn relay");
    proc.stdin
        .as_mut()
        .expect("stdin")
        .write_all(br#"{"whatever":1}"#)
        .expect("write");
    let out = proc.wait_with_output().expect("wait");

    // ソケットが無くても debug 記録は残り、exit code は 0 のまま。
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    assert!(out.stderr.is_empty());

    let entries: Vec<_> = std::fs::read_dir(&dir)
        .expect("debug dir must exist")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "expected one capture file, got {entries:?}"
    );
    assert!(
        entries[0].ends_with("-Notification.json"),
        "unexpected name: {}",
        entries[0]
    );

    let content = std::fs::read_to_string(dir.join(&entries[0])).expect("read capture");
    assert_eq!(content, r#"{"whatever":1}"#);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_debug_dir_means_no_files() {
    let dir = std::env::temp_dir().join(format!("kamux-relay-nodebug-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let out = Command::new(relay_bin())
        .arg("Stop")
        .stdin(Stdio::null())
        .env("KAMUX_SESSION_ID", "3f2a0000-0000-4000-8000-000000009c1e")
        .env("KAMUX_HOOKS_SOCK", "/nonexistent/kamux-hooks-none.sock")
        .env_remove("KAMUX_RELAY_DEBUG_DIR")
        .output()
        .expect("spawn relay");

    assert_eq!(out.status.code(), Some(0));
    assert!(
        !dir.exists(),
        "debug dir must not be created when the env var is unset"
    );
}

/// `KAMUX_RELAY_DEBUG_DIR` の親を辿れない場合（`/dev/null` 配下はディレクトリに
/// できない）でも、relay 本来の役目であるソケットへの転送は行われなければならない。
#[test]
fn debug_mkdir_failure_does_not_block_socket_forward() {
    let sock = std::env::temp_dir().join(format!(
        "kamux-relay-debugfail-mkdir-{}.sock",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&sock);
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
            .env("KAMUX_RELAY_DEBUG_DIR", "/dev/null/kamux-relay-debug-fail")
            .spawn()
            .expect("spawn relay");
        proc.stdin
            .as_mut()
            .expect("stdin")
            .write_all(br#"{"whatever":1}"#)
            .expect("write");
        proc.wait_with_output().expect("wait")
    });

    let buf = accept_and_read_with_timeout(listener);

    let out = child.join().expect("join");
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    assert!(out.stderr.is_empty());

    let v: serde_json::Value = serde_json::from_str(&buf).expect("valid JSON on the wire");
    assert_eq!(v["hook_kind"], "Notification");

    let _ = std::fs::remove_file(&sock);
}

/// `KAMUX_RELAY_DEBUG_DIR` 自体は存在し `create_dir_all` は成功するが、書き込み
/// 権限が無く `fs::write` が失敗するケース。この場合も relay 本来の役目である
/// ソケットへの転送は行われなければならない。
#[test]
fn debug_write_permission_failure_does_not_block_socket_forward() {
    let pid = std::process::id();
    let sock = std::env::temp_dir().join(format!("kamux-relay-debugfail-perm-{pid}.sock"));
    let dir = std::env::temp_dir().join(format!("kamux-relay-debugfail-perm-dir-{pid}"));
    let _ = std::fs::remove_file(&sock);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create readonly dir");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).expect("chmod readonly");
    let listener = UnixListener::bind(&sock).expect("bind");

    let sock_for_child = sock.clone();
    let dir_for_child = dir.clone();
    let child = std::thread::spawn(move || {
        let mut proc = Command::new(relay_bin())
            .arg("Notification")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("KAMUX_SESSION_ID", "3f2a0000-0000-4000-8000-000000009c1e")
            .env("KAMUX_HOOKS_SOCK", &sock_for_child)
            .env("KAMUX_RELAY_DEBUG_DIR", &dir_for_child)
            .spawn()
            .expect("spawn relay");
        proc.stdin
            .as_mut()
            .expect("stdin")
            .write_all(br#"{"whatever":1}"#)
            .expect("write");
        proc.wait_with_output().expect("wait")
    });

    let buf = accept_and_read_with_timeout(listener);

    let out = child.join().expect("join");
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    assert!(out.stderr.is_empty());

    let v: serde_json::Value = serde_json::from_str(&buf).expect("valid JSON on the wire");
    assert_eq!(v["hook_kind"], "Notification");

    // 書き込みできないのでキャプチャファイルは 1 個も無いはず。
    let entries: Vec<_> = std::fs::read_dir(&dir)
        .expect("dir must still exist")
        .filter_map(|e| e.ok())
        .collect();
    assert!(
        entries.is_empty(),
        "read-only dir must not receive a capture file, got {entries:?}"
    );

    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&sock);
}
