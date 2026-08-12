//! 設定画面向けの hooks 疎通ステータス（設計書 §12）。
//!
//! `liveness` はサーバ側でオンデマンド計算されるため、呼んだ瞬間の正しい値が返る。
//! フロントはパネルを開いたときと `session://state` を受けたときにだけ呼ぶ。定期リフレッシュはしない。

use serde::Serialize;

use super::registry::SessionHookStatus;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HooksDiagnostics {
    /// 契約 §12 の `$TMPDIR/kamux-hooks-{pid}.sock`
    pub socket_path: String,
    /// Unix ソケットリスナが生きているか。セッション単位の liveness とは独立
    pub listener_alive: bool,
    /// `session_id` 昇順。UI の行が並び替わらないように固定する
    pub sessions: Vec<SessionHookStatus>,
}

pub fn build_diagnostics(
    socket_path: String,
    listener_alive: bool,
    mut sessions: Vec<SessionHookStatus>,
) -> HooksDiagnostics {
    sessions.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    HooksDiagnostics {
        socket_path,
        listener_alive,
        sessions,
    }
}

/// `AppState` から `HooksDiagnostics` を組み立てる。`#[tauri::command]` 本体は
/// `State<'_, AppState>` をテストから構築できないため、状態を読む部分はここへ
/// 出す（`lib.rs` 冒頭のコメント: 各コマンドは Store への薄いラッパに徹する）。
///
/// - `hooks_server` に生きたサーバがあれば、そのサーバの `socket_path()` /
///   `is_alive()` を真実として使う（実際に bind されている path のほうが
///   宣言上の path より確からしい）。
/// - サーバが無い、またはロックが poisoned なら `listener_alive` は常に
///   `false`。`socket_path` は `state.hooks` が持つ宣言上の path、それも
///   無ければ空文字列。
/// - `sessions` は `state.heuristics.diagnostics()` をそのまま `build_diagnostics`
///   へ渡す。
pub fn diagnostics_from_state(state: &crate::state::AppState) -> HooksDiagnostics {
    let declared_socket_path = || {
        state
            .hooks
            .as_ref()
            .map(|h| h.socket_path.to_string_lossy().into_owned())
            .unwrap_or_default()
    };

    let (socket_path, listener_alive) = match state.hooks_server.lock() {
        Ok(guard) => match guard.as_ref() {
            Some(server) => (
                server.socket_path().to_string_lossy().into_owned(),
                server.is_alive(),
            ),
            None => (declared_socket_path(), false),
        },
        Err(_) => (declared_socket_path(), false),
    };

    build_diagnostics(socket_path, listener_alive, state.heuristics.diagnostics())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CliKind;
    use crate::session::heuristics::hook_liveness::HookLiveness;

    fn status(id: &str, liveness: HookLiveness, active: bool) -> SessionHookStatus {
        SessionHookStatus {
            session_id: id.to_string(),
            cli_kind: CliKind::Claude,
            liveness,
            last_hook_at: None,
            heuristics_active: active,
        }
    }

    #[test]
    fn carries_the_socket_path_and_listener_state() {
        let d = build_diagnostics("/tmp/kamux-hooks-42.sock".into(), true, vec![]);
        assert_eq!(d.socket_path, "/tmp/kamux-hooks-42.sock");
        assert!(d.listener_alive);
        assert!(d.sessions.is_empty());
    }

    #[test]
    fn sessions_are_sorted_by_id_for_a_stable_ui() {
        let d = build_diagnostics(
            "/tmp/s.sock".into(),
            true,
            vec![
                status("s3", HookLiveness::Healthy, false),
                status("s1", HookLiveness::Pending, false),
                status("s2", HookLiveness::Unreachable, true),
            ],
        );
        let ids: Vec<_> = d.sessions.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, vec!["s1", "s2", "s3"]);
    }

    #[test]
    fn serializes_with_snake_case_field_names() {
        let d = build_diagnostics(
            "/tmp/s.sock".into(),
            false,
            vec![status("s1", HookLiveness::Unreachable, true)],
        );
        let json = serde_json::to_string(&d).expect("serialize");
        assert!(json.contains("\"socket_path\""));
        assert!(json.contains("\"listener_alive\":false"));
        assert!(json.contains("\"session_id\":\"s1\""));
        assert!(json.contains("\"liveness\":\"unreachable\""));
        assert!(json.contains("\"heuristics_active\":true"));
        assert!(json.contains("\"last_hook_at\":null"));
    }

    #[test]
    fn a_dead_listener_is_reported_without_hiding_sessions() {
        let d = build_diagnostics(
            "/tmp/s.sock".into(),
            false,
            vec![status("s1", HookLiveness::Healthy, false)],
        );
        assert!(!d.listener_alive);
        assert_eq!(
            d.sessions.len(),
            1,
            "リスナが死んでもセッション行は隠さない"
        );
    }

    // --- Task 14 レビュー（lane-controller 裁定）: `build_diagnostics` は
    // ソートして包むだけの純関数で、`AppState` を実際に読む `diagnostics_from_state`
    // には観測が無い。以下はそちらを固定する。

    use std::sync::Arc;

    use crate::hooks_srv::{HookEvent, HookSink, HooksRuntime, HooksServer};

    struct NoopHookSink;
    impl HookSink for NoopHookSink {
        fn on_hook(&self, _event: HookEvent) {}
    }

    fn test_diag_socket_path(tag: &str) -> std::path::PathBuf {
        let p =
            std::env::temp_dir().join(format!("kamux-diagtest-{tag}-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn unused_hooks_runtime(tag: &str, socket_path: std::path::PathBuf) -> HooksRuntime {
        let dir = std::env::temp_dir();
        let pid = std::process::id();
        HooksRuntime {
            socket_path,
            settings_path: dir.join(format!("kamux-diagtest-{tag}-{pid}-settings.json")),
            relay_bin: dir.join(format!("kamux-diagtest-{tag}-{pid}-relay")),
        }
    }

    /// サーバが生きているとき、宣言上の（`state.hooks`）path ではなく実際に
    /// bind されているサーバの path が真実として使われることを固定する。
    /// わざと `state.hooks` の path をサーバの path とずらし、混同していれば
    /// このテストが検出する。
    #[test]
    fn listener_alive_and_socket_path_reflect_a_running_hooks_server() {
        let (_dir, store) = crate::store::test_support::open_temp();
        let mut state = crate::state::test_support::app_state(store);

        let declared_but_unused = test_diag_socket_path("declared-mismatch");
        state.hooks = Some(unused_hooks_runtime("mismatch", declared_but_unused));

        let real_sock = test_diag_socket_path("alive");
        let server =
            HooksServer::start(real_sock.clone(), Arc::new(NoopHookSink)).expect("start server");
        state.hooks_server = std::sync::Mutex::new(Some(server));

        let d = diagnostics_from_state(&state);
        assert!(d.listener_alive);
        assert_eq!(d.socket_path, real_sock.to_string_lossy());
    }

    #[test]
    fn listener_alive_is_false_once_the_hooks_server_is_shut_down() {
        let (_dir, store) = crate::store::test_support::open_temp();
        let mut state = crate::state::test_support::app_state(store);

        let sock = test_diag_socket_path("shutdown");
        let mut server =
            HooksServer::start(sock.clone(), Arc::new(NoopHookSink)).expect("start server");
        server.shutdown();
        state.hooks_server = std::sync::Mutex::new(Some(server));

        let d = diagnostics_from_state(&state);
        assert!(!d.listener_alive);
        assert_eq!(
            d.socket_path,
            sock.to_string_lossy(),
            "shutdown 後も socket_path は変わらない"
        );
    }

    #[test]
    fn hooks_and_hooks_server_absent_yield_a_dead_listener_and_empty_socket_path() {
        let (_dir, store) = crate::store::test_support::open_temp();
        let state = crate::state::test_support::app_state(store);

        let d = diagnostics_from_state(&state);
        assert!(!d.listener_alive);
        assert_eq!(d.socket_path, "");
    }

    #[test]
    fn hooks_present_without_a_server_reports_the_declared_socket_path_with_a_dead_listener() {
        let (_dir, store) = crate::store::test_support::open_temp();
        let mut state = crate::state::test_support::app_state(store);

        let declared = test_diag_socket_path("declared-only");
        state.hooks = Some(unused_hooks_runtime("declared-only", declared.clone()));

        let d = diagnostics_from_state(&state);
        assert!(!d.listener_alive);
        assert_eq!(d.socket_path, declared.to_string_lossy());
    }

    /// レビュー Important 1（task-14-review.md）: `hooks_server.lock()` が
    /// poisoned（保持中のスレッドが panic した）とき、`Err(_)` 分岐が実際に
    /// 踏まれ `listener_alive == false` / `socket_path` は宣言上の path へ
    /// フォールバックすることを固定する。値を差し替える変異
    /// （`(declared_socket_path(), false)` → 別の値）はこれまで検出されなかった
    /// （9/9 全緑）。
    ///
    /// 陽性の対照を同じフィクスチャに同居させる: 毒される前は実サーバが生きて
    /// おり `listener_alive == true` になることも固定する。これが無いと
    /// 「常に false を返す実装」でもこのテストが緑になり判別力が無い。
    #[test]
    fn a_poisoned_hooks_server_lock_reports_a_dead_listener_via_the_declared_path() {
        let (_dir, store) = crate::store::test_support::open_temp();
        let mut state = crate::state::test_support::app_state(store);

        let declared = test_diag_socket_path("poisoned-declared");
        state.hooks = Some(unused_hooks_runtime("poisoned", declared.clone()));

        let real_sock = test_diag_socket_path("poisoned-real");
        let server =
            HooksServer::start(real_sock.clone(), Arc::new(NoopHookSink)).expect("start server");
        state.hooks_server = std::sync::Mutex::new(Some(server));

        // 陽性の対照: 毒される前は listener_alive == true（サーバの実 path が真実）。
        let before = diagnostics_from_state(&state);
        assert!(before.listener_alive);
        assert_eq!(before.socket_path, real_sock.to_string_lossy());

        // `hooks_server.lock()` を保持したまま panic させ、poisoned にする。
        // 子スレッドの panic はテスト自体を落とさない。
        let state = Arc::new(state);
        let state_for_thread = Arc::clone(&state);
        let poisoning = std::thread::spawn(move || {
            let _guard = state_for_thread.hooks_server.lock().expect("lock");
            panic!("poison the hooks_server mutex on purpose");
        });
        assert!(
            poisoning.join().is_err(),
            "spawned thread must have panicked while holding the lock"
        );

        let after = diagnostics_from_state(&state);
        assert!(
            !after.listener_alive,
            "a poisoned lock must report a dead listener"
        );
        assert_eq!(
            after.socket_path,
            declared.to_string_lossy(),
            "a poisoned lock must fall back to the declared path in state.hooks"
        );
    }

    #[test]
    fn sessions_in_diagnostics_come_from_the_heuristics_registry() {
        let (_dir, store) = crate::store::test_support::open_temp();
        let state = crate::state::test_support::app_state(store);

        state
            .heuristics
            .register("sess-b", CliKind::Claude, true, 30);
        state
            .heuristics
            .register("sess-a", CliKind::Claude, true, 30);

        let d = diagnostics_from_state(&state);
        let ids: Vec<_> = d.sessions.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, vec!["sess-a", "sess-b"]);
    }
}
