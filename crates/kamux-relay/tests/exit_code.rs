use std::process::{Command, Stdio};

fn relay_bin() -> &'static str {
    env!("CARGO_BIN_EXE_kamux-relay")
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
