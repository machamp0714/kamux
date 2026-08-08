//! claude の hook コマンドとして起動される使い捨てプロセス。
//!
//! 契約 §12.3: 何があっても exit 0、stdout/stderr には何も書かない。
//! exit 2 を返すとユーザーの claude セッションが停止するため、
//! すべての失敗経路を Option の早期 return で握りつぶす。

mod wire;

use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// hook payload の受け入れ上限。Stop の last_assistant_message は長くなりうる。
const MAX_STDIN_BYTES: u64 = 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(2);

fn main() {
    // panic メッセージが PTY 内の claude 表示を汚さないよう、既定のフックを潰す。
    std::panic::set_hook(Box::new(|_| {}));
    let _ = std::panic::catch_unwind(run);
    std::process::exit(0);
}

fn run() {
    let _ = relay();
}

/// 失敗はすべて None。呼び出し側は無視する。
fn relay() -> Option<()> {
    let hook_kind = std::env::args().nth(1)?;
    if !is_valid_hook_kind(&hook_kind) {
        return None;
    }

    let kamux_session_id = std::env::var("KAMUX_SESSION_ID").ok()?;
    if !wire::is_valid_session_id(&kamux_session_id) {
        return None;
    }

    let socket_path = std::env::var("KAMUX_HOOKS_SOCK").ok()?;

    let mut raw = Vec::new();
    std::io::stdin()
        .take(MAX_STDIN_BYTES)
        .read_to_end(&mut raw)
        .ok()?;
    // take() は上限に当たっても Ok を返す。切り詰めを検知して下流に伝える。
    let truncated = raw.len() as u64 == MAX_STDIN_BYTES;

    let message = wire::build_wire_message(&kamux_session_id, &hook_kind, &raw, truncated);

    let mut stream = UnixStream::connect(&socket_path).ok()?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).ok()?;
    stream.write_all(message.as_bytes()).ok()?;
    stream.flush().ok()?;
    // close で EOF を送り、サーバ側の read_to_end を終わらせる。
    stream.shutdown(Shutdown::Write).ok()?;
    Some(())
}

/// argv[1] は kamux 自身が settings JSON に書いた値だが、念のため形を検査する。
fn is_valid_hook_kind(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s.chars().all(|c| c.is_ascii_alphanumeric())
}
