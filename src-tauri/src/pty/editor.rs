use crate::pty::launch_env::LaunchEnv;

/// ログインシェルのロケールが UTF-8 でないときの既定。
pub const EDITOR_FALLBACK_LANG: &str = "en_US.UTF-8";

// EDITOR_TERM / EDITOR_COLORTERM は**置かない**(契約 §60.6 / §60.8)。
// TERM / COLORTERM の所有は PtySurface::spawn であり、src-tauri/src/pty/surface.rs の
// `cmd.env("TERM", …)` の隣に 1 行ずつ並ぶ。ここに定数を置くと真実源が 2 箇所になり、
// spec.env が後から適用される(cmd.env(key, value) のループがそれより後にある)ぶん、
// 契約に行を持たない側が黙って勝つ。
// 検査 10(契約 §60.6.4)が「TERM / COLORTERM を設定するファイルはちょうど 1 つ」を見ている。

/// nvim 用 PTY に注入する環境変数(契約 §15: 既存環境を継承した上で上書きされる)。
///
/// PATH と LANG は契約 §18 の `probe_login_env()` が探査したもの。M3-1 は探査しない。
/// KAMUX_SESSION_ID は意図的に入れない(設計判断 D11)。
/// TERM / COLORTERM は入れない(契約 §60.6。所有は `PtySurface::spawn`)。
///
/// LANG の UTF-8 判定を残しているのは、§18 のシステムロケール導出が働くのが
/// 「探査値が空のとき」だけで、LANG=C を明示している環境では C がそのまま来るため
/// (C ロケールの nvim は日本語ファイル名・日本語テキストを化けさせる。設計判断 D6)。
pub fn build_editor_env(launch: &LaunchEnv) -> Vec<(String, String)> {
    let lang = if launch
        .lang
        .to_ascii_uppercase()
        .replace('-', "")
        .contains("UTF8")
    {
        launch.lang.clone()
    } else {
        EDITOR_FALLBACK_LANG.to_string()
    };
    vec![
        ("PATH".to_string(), launch.path.clone()),
        ("LANG".to_string(), lang),
    ]
}

/// 同時に live にできる nvim サーフェスの上限(契約 §19 / 設計判断 D3)。
/// LRU で自動 kill しないのは、未保存バッファを黙って捨てないため。
pub const MAX_LIVE_EDITOR_SURFACES: usize = 3;

/// 契約 §5 の surface_id 形式 "{session_id}:{surface_kind}" に対する判定。
pub fn is_editor_surface(surface_id: &str) -> bool {
    surface_id.ends_with(":editor")
}

pub fn count_live_editor_surfaces(surface_ids: &[String]) -> usize {
    surface_ids.iter().filter(|s| is_editor_surface(s)).count()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorSpawnDecision {
    AlreadyLive,
    Spawn,
    LimitReached,
}

/// live は `PtyManager::live_surfaces()` の戻り値(契約 §15)。
pub fn decide_editor_spawn(target: &str, live: &[String], max: usize) -> EditorSpawnDecision {
    if live.iter().any(|s| s == target) {
        return EditorSpawnDecision::AlreadyLive;
    }
    if count_live_editor_surfaces(live) >= max {
        return EditorSpawnDecision::LimitReached;
    }
    EditorSpawnDecision::Spawn
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::launch_env::LaunchEnv;

    fn env_of(env: &[(String, String)], key: &str) -> Option<String> {
        env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    /// 契約 §18 の probe_login_env() が返す値の代わり。純粋関数なので実探査は不要。
    fn launch(path: &str, lang: &str) -> LaunchEnv {
        LaunchEnv {
            path: path.to_string(),
            lang: lang.to_string(),
        }
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // ---- build_editor_env(設計判断 D6 / D11) ----

    #[test]
    fn env_passes_through_probed_path_and_never_declares_terminal_type() {
        let env = build_editor_env(&launch("/opt/homebrew/bin:/usr/bin", "ja_JP.UTF-8"));
        assert_eq!(
            env_of(&env, "PATH").as_deref(),
            Some("/opt/homebrew/bin:/usr/bin")
        );
        // 契約 §60.6: TERM / COLORTERM の所有は PtySurface::spawn。ここには現れない。
        // (元は「xterm-256color / truecolor が入る」ことを見る 2 assert だった。§60.8 で反転した)
        assert_eq!(env_of(&env, "TERM"), None);
        assert_eq!(env_of(&env, "COLORTERM"), None);
    }

    #[test]
    fn env_never_injects_kamux_session_id() {
        // エディタは hooks に関与しない(設計判断 D11)
        let env = build_editor_env(&launch("/usr/bin", "ja_JP.UTF-8"));
        assert_eq!(env_of(&env, "KAMUX_SESSION_ID"), None);
    }

    #[test]
    fn keeps_utf8_locale_from_login_shell() {
        assert_eq!(
            env_of(
                &build_editor_env(&launch("/usr/bin", "ja_JP.UTF-8")),
                "LANG"
            )
            .as_deref(),
            Some("ja_JP.UTF-8")
        );
        assert_eq!(
            env_of(&build_editor_env(&launch("/usr/bin", "en_US.utf8")), "LANG").as_deref(),
            Some("en_US.utf8")
        );
    }

    #[test]
    fn falls_back_when_the_locale_is_not_utf8() {
        // 契約 §18 は lang が非空であることを保証するが、ログインシェルが
        // LANG=C を明示している環境ではその値がそのまま来る。C ロケールの
        // nvim は日本語を化けさせるので、ここで UTF-8 に落とす
        assert_eq!(
            env_of(&build_editor_env(&launch("/usr/bin", "C")), "LANG").as_deref(),
            Some("en_US.UTF-8")
        );
        assert_eq!(
            env_of(&build_editor_env(&launch("/usr/bin", "POSIX")), "LANG").as_deref(),
            Some("en_US.UTF-8")
        );
        // §18 が壊れて空文字が来ても化けさせない
        assert_eq!(
            env_of(&build_editor_env(&launch("/usr/bin", "")), "LANG").as_deref(),
            Some("en_US.UTF-8")
        );
    }

    // ---- spawn 判定(設計判断 D3) ----

    #[test]
    fn only_editor_suffixed_surfaces_count() {
        assert!(is_editor_surface("abc:editor"));
        assert!(!is_editor_surface("abc:agent"));
        assert!(!is_editor_surface("editor"));
        let live = ids(&["a:agent", "a:editor", "b:agent", "b:editor", "c:agent"]);
        assert_eq!(count_live_editor_surfaces(&live), 2);
    }

    #[test]
    fn already_live_target_is_idempotent() {
        let live = ids(&["a:editor", "b:editor", "c:editor"]);
        assert_eq!(
            decide_editor_spawn("b:editor", &live, MAX_LIVE_EDITOR_SURFACES),
            EditorSpawnDecision::AlreadyLive
        );
    }

    #[test]
    fn spawns_while_under_the_limit() {
        let live = ids(&["a:editor", "b:editor"]);
        assert_eq!(
            decide_editor_spawn("c:editor", &live, 3),
            EditorSpawnDecision::Spawn
        );
    }

    #[test]
    fn refuses_at_the_limit() {
        let live = ids(&["a:editor", "b:editor", "c:editor"]);
        assert_eq!(
            decide_editor_spawn("d:editor", &live, 3),
            EditorSpawnDecision::LimitReached
        );
    }

    #[test]
    fn agent_surfaces_do_not_consume_the_editor_budget() {
        let live = ids(&["a:agent", "b:agent", "c:agent", "d:agent", "e:agent"]);
        assert_eq!(
            decide_editor_spawn("a:editor", &live, 3),
            EditorSpawnDecision::Spawn
        );
    }

    #[test]
    fn decision_is_only_the_first_gate() {
        // live のスナップショットは判定時点のもの。同一セッションへの並行 spawn_editor は
        // 両方が Spawn を得うるので、呼び出し側は spawn 直前に is_alive で再確認する
        // (Task 4 参照。契約 §15 の spawn は生存中の同 surface_id に InvalidState を返す)
        let live: Vec<String> = Vec::new();
        assert_eq!(
            decide_editor_spawn("a:editor", &live, 3),
            EditorSpawnDecision::Spawn
        );
        assert_eq!(
            decide_editor_spawn("a:editor", &live, 3),
            EditorSpawnDecision::Spawn
        );
    }

    // cwd 算出のテストはここには置かない。契約 §23 のとおり `resolve_cwd` は
    // `session/cli_args.rs`(M1-3 の所有物)にあり、そちらでテスト済みのため。
}
