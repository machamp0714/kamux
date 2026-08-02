/// CLI を直接 exec する経路に注入する環境（契約 §18）。
/// M1-4 / M3-1 がこのファイルに `probe_login_env()`（`$SHELL -ilc` による探査）と
/// `resolve_program()` を追加する。M1-3 は `$SHELL -l` しか起動しないため探査は不要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchEnv {
    /// ログインシェルから取得した PATH
    pub path: String,
    /// ログインシェルから取得した LANG。空だと nvim で日本語ファイル名が化ける
    pub lang: String,
}

impl LaunchEnv {
    /// 現プロセスの環境をそのまま使う暫定コンストラクタ。
    /// GUI 起動では PATH が不完全になるため、M1-4 が `probe_login_env()` に差し替える。
    pub fn from_current_process() -> Self {
        Self {
            path: std::env::var("PATH").unwrap_or_default(),
            lang: std::env::var("LANG").unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // PATH は cargo test の実行環境で必ず非空なので、path/lang の取り違え変異を
    // 環境変数の書き換えなしに検出できる（並列テストと干渉しない）。
    #[test]
    fn from_current_process_reads_path_into_the_path_field_not_lang() {
        let env = LaunchEnv::from_current_process();
        assert_eq!(env.path, std::env::var("PATH").unwrap_or_default());
    }

    #[test]
    fn from_current_process_reads_lang_into_the_lang_field_not_path() {
        let env = LaunchEnv::from_current_process();
        assert_eq!(env.lang, std::env::var("LANG").unwrap_or_default());
    }
}
