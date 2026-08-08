//! claude の hook コマンドとして起動される使い捨てプロセス。
//! 契約 §12.3: 何があっても exit 0、stdout/stderr には何も書かない。

fn main() {
    // panic メッセージが PTY 内の claude 表示を汚さないよう、既定のフックを潰す。
    std::panic::set_hook(Box::new(|_| {}));
    let _ = std::panic::catch_unwind(run);
    std::process::exit(0);
}

fn run() {}
